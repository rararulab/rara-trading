//! Template renderer for LLM prompts loaded from TOML files.
//!
//! Templates use `{variable}` placeholders that are substituted at render time.

use std::{collections::HashMap, path::Path};

use snafu::{ResultExt, Snafu};

/// Errors from the prompt renderer.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum PromptError {
    /// The requested template was not found.
    #[snafu(display("template not found: {name}"))]
    TemplateNotFound {
        /// Name of the missing template.
        name: String,
    },
    /// Failed to parse a TOML template file.
    #[snafu(display("failed to parse template: {source}"))]
    Parse {
        /// The underlying TOML parse error.
        source: toml::de::Error,
    },
    /// IO error reading template files.
    #[snafu(display("IO error reading templates: {source}"))]
    Io {
        /// The underlying IO error.
        source: std::io::Error,
    },
    /// A required variable was missing during rendering.
    #[snafu(display("missing variable '{name}' in template '{template}'"))]
    MissingVariable {
        /// Name of the missing variable.
        name:     String,
        /// Name of the template being rendered.
        template: String,
    },
}

/// Alias for prompt renderer results.
pub type Result<T> = std::result::Result<T, PromptError>;

/// Internal structure matching the TOML file format.
#[derive(serde::Deserialize)]
struct TemplateFile {
    template: TemplateContent,
}

/// The `[template]` section of a TOML file.
#[derive(serde::Deserialize)]
struct TemplateContent {
    name:   String,
    prompt: String,
}

/// Renders prompt templates loaded from TOML files.
///
/// Templates use `{variable}` syntax for placeholder substitution.
/// Literal braces can be escaped as `{{` and `}}`.
pub struct PromptRenderer {
    templates: HashMap<String, String>,
}

impl PromptRenderer {
    /// Load all `.toml` templates from the given directory.
    ///
    /// Each file must contain a `[template]` section with `name` and `prompt`
    /// fields.
    pub fn load_from_dir(dir: &Path) -> Result<Self> {
        let mut templates = HashMap::new();

        let entries = std::fs::read_dir(dir).context(IoSnafu)?;

        for entry in entries {
            let entry = entry.context(IoSnafu)?;
            let path = entry.path();

            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }

            let content = std::fs::read_to_string(&path).context(IoSnafu)?;
            let parsed: TemplateFile = toml::from_str(&content).context(ParseSnafu)?;

            templates.insert(parsed.template.name, parsed.template.prompt);
        }

        Ok(Self { templates })
    }

    /// Create a renderer from an in-memory map of templates.
    ///
    /// Useful for testing without filesystem access.
    pub const fn from_map(templates: HashMap<String, String>) -> Self { Self { templates } }

    /// Render a named template with the given variable substitutions.
    ///
    /// Escaped braces (`{{` / `}}`) are preserved as literal `{` / `}`.
    /// Missing variables produce an error.
    pub fn render(&self, template_name: &str, vars: &HashMap<String, String>) -> Result<String> {
        let template = self.templates.get(template_name).ok_or_else(|| {
            TemplateNotFoundSnafu {
                name: template_name.to_owned(),
            }
            .build()
        })?;

        substitute(template, vars, template_name)
    }

    /// List all loaded template names.
    pub fn template_names(&self) -> Vec<&str> {
        self.templates.keys().map(String::as_str).collect()
    }
}

/// Perform `{variable}` substitution on a template string.
///
/// `{{` and `}}` are treated as escaped literal braces.
/// Unmatched `{name}` placeholders with no corresponding variable produce an
/// error.
fn substitute(
    template: &str,
    vars: &HashMap<String, String>,
    template_name: &str,
) -> Result<String> {
    let mut result = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '{' => {
                if chars.peek() == Some(&'{') {
                    // Escaped opening brace
                    chars.next();
                    result.push('{');
                } else {
                    // Collect variable name until closing brace
                    let mut var_name = String::new();
                    for inner in chars.by_ref() {
                        if inner == '}' {
                            break;
                        }
                        var_name.push(inner);
                    }
                    match vars.get(&var_name) {
                        Some(value) => result.push_str(value),
                        None => {
                            return Err(MissingVariableSnafu {
                                name:     var_name,
                                template: template_name.to_owned(),
                            }
                            .build());
                        }
                    }
                }
            }
            '}' => {
                if chars.peek() == Some(&'}') {
                    // Escaped closing brace
                    chars.next();
                    result.push('}');
                } else {
                    result.push(ch);
                }
            }
            _ => result.push(ch),
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_renderer(name: &str, prompt: &str) -> PromptRenderer {
        let mut map = HashMap::new();
        map.insert(name.to_owned(), prompt.to_owned());
        PromptRenderer::from_map(map)
    }

    #[test]
    fn render_substitutes_all_variables() {
        let renderer = make_renderer("test", "Hello {name}, your score is {score}.");
        let mut vars = HashMap::new();
        vars.insert("name".to_owned(), "Alice".to_owned());
        vars.insert("score".to_owned(), "42".to_owned());

        let result = renderer.render("test", &vars).unwrap();
        assert_eq!(result, "Hello Alice, your score is 42.");
    }

    #[test]
    fn render_missing_variable_returns_error() {
        let renderer = make_renderer("test", "Hello {name}.");
        let vars = HashMap::new();

        let err = renderer.render("test", &vars).unwrap_err();
        assert!(err.to_string().contains("missing variable 'name'"));
    }

    #[test]
    fn render_template_not_found() {
        let renderer = make_renderer("test", "irrelevant");
        let vars = HashMap::new();

        let err = renderer.render("nonexistent", &vars).unwrap_err();
        assert!(err.to_string().contains("template not found: nonexistent"));
    }

    #[test]
    fn render_escaped_braces() {
        let renderer = make_renderer("test", "JSON: {{\"key\": \"{value}\"}}");
        let mut vars = HashMap::new();
        vars.insert("value".to_owned(), "hello".to_owned());

        let result = renderer.render("test", &vars).unwrap();
        assert_eq!(result, "JSON: {\"key\": \"hello\"}");
    }

    #[test]
    fn render_empty_vars_on_no_placeholders() {
        let renderer = make_renderer("test", "No placeholders here.");
        let vars = HashMap::new();

        let result = renderer.render("test", &vars).unwrap();
        assert_eq!(result, "No placeholders here.");
    }

    #[test]
    fn load_from_dir_reads_toml_files() {
        let dir = tempfile::tempdir().unwrap();
        let toml_content = r#"
[template]
name = "greeting"
prompt = "Hello {who}!"
"#;
        std::fs::write(dir.path().join("greeting.toml"), toml_content).unwrap();

        // Non-toml file should be ignored
        std::fs::write(dir.path().join("readme.txt"), "not a template").unwrap();

        let renderer = PromptRenderer::load_from_dir(dir.path()).unwrap();

        let mut vars = HashMap::new();
        vars.insert("who".to_owned(), "world".to_owned());

        let result = renderer.render("greeting", &vars).unwrap();
        assert_eq!(result, "Hello world!");
    }

    #[test]
    fn load_from_dir_multiple_templates() {
        let dir = tempfile::tempdir().unwrap();

        std::fs::write(
            dir.path().join("a.toml"),
            "[template]\nname = \"alpha\"\nprompt = \"A: {x}\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("b.toml"),
            "[template]\nname = \"beta\"\nprompt = \"B: {y}\"\n",
        )
        .unwrap();

        let renderer = PromptRenderer::load_from_dir(dir.path()).unwrap();

        let mut names = renderer.template_names();
        names.sort_unstable();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn render_hypothesis_gen_template_shape() {
        // Verify the actual hypothesis_gen template can be rendered
        let prompt_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/prompts");
        let renderer = PromptRenderer::load_from_dir(&prompt_dir).unwrap();

        let mut vars = HashMap::new();
        vars.insert("trace_history".to_owned(), "[Iteration 0] ...".to_owned());
        vars.insert("sota_result".to_owned(), "Sharpe: 1.5".to_owned());
        vars.insert("last_feedback".to_owned(), "Good momentum".to_owned());

        let result = renderer.render("hypothesis_gen", &vars).unwrap();
        assert!(result.contains("[Iteration 0] ..."));
        assert!(result.contains("Sharpe: 1.5"));
        assert!(result.contains("Good momentum"));
        assert!(result.contains("hypothesis"));
    }

    #[test]
    fn render_strategy_code_template_shape() {
        let prompt_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/prompts");
        let renderer = PromptRenderer::load_from_dir(&prompt_dir).unwrap();

        let mut vars = HashMap::new();
        vars.insert("hypothesis".to_owned(), "Use RSI crossover".to_owned());
        vars.insert(
            "strategy_trait".to_owned(),
            "trait TradingStrategy { ... }".to_owned(),
        );
        vars.insert("prior_code".to_owned(), "fn old() {}".to_owned());
        vars.insert("compile_errors".to_owned(), "none".to_owned());

        let result = renderer.render("strategy_code", &vars).unwrap();
        assert!(result.contains("Use RSI crossover"));
        assert!(result.contains("fn meta()"));
    }

    #[test]
    fn render_feedback_gen_template_shape() {
        let prompt_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/prompts");
        let renderer = PromptRenderer::load_from_dir(&prompt_dir).unwrap();

        let mut vars = HashMap::new();
        vars.insert("hypothesis".to_owned(), "Mean reversion works".to_owned());
        vars.insert(
            "backtest_result".to_owned(),
            "Sharpe: 2.0, PnL: 500".to_owned(),
        );
        vars.insert("sota_result".to_owned(), "Sharpe: 1.5, PnL: 300".to_owned());
        vars.insert("strategy_code".to_owned(), "fn strategy() {}".to_owned());

        let result = renderer.render("feedback_gen", &vars).unwrap();
        assert!(result.contains("Mean reversion works"));
        assert!(result.contains("decision"));
    }
}
