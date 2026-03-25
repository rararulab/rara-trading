//! Hot-to-cold archival: converts old `.rara` binary files into Parquet.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use arrow::array::{Int64Array, UInt32Array};
use arrow::datatypes::{DataType as ArrowDataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use jiff::{civil::Date, Zoned};
use parquet::arrow::ArrowWriter;
use snafu::{ResultExt, Snafu};

use crate::file::{FileError, RaraFileReader};
use crate::record::CandleRecord;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors produced during hot-to-cold archival.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum ArchiverError {
    /// An I/O error occurred.
    #[snafu(display("I/O error: {source}"))]
    Io {
        /// Underlying OS error.
        source: std::io::Error,
    },

    /// A `.rara` file operation failed.
    #[snafu(display("file error: {source}"))]
    File {
        /// Underlying file error.
        source: FileError,
    },

    /// Arrow record batch construction failed.
    #[snafu(display("arrow error: {message}"))]
    Arrow {
        /// Error message from Arrow.
        message: String,
    },

    /// Parquet writing failed.
    #[snafu(display("parquet error: {message}"))]
    Parquet {
        /// Error message from Parquet.
        message: String,
    },
}

/// Convenience alias for archiver operations.
pub type Result<T> = std::result::Result<T, ArchiverError>;

// ---------------------------------------------------------------------------
// Archiver
// ---------------------------------------------------------------------------

/// Converts hot `.rara` binary files to cold Parquet archives.
#[derive(bon::Builder)]
pub struct Archiver {
    /// Root data directory containing `hot/` and `cold/` subdirectories.
    data_dir: PathBuf,
    /// Files older than this many days are eligible for archival.
    #[builder(default = 7)]
    retention_days: u32,
}

impl Archiver {
    /// Archive old `.rara` files for a specific instrument and data type.
    ///
    /// Scans `hot/{instrument_id}/{data_type_dir}/` for `.rara` files older than
    /// `retention_days`, groups by month, writes Parquet to
    /// `cold/{instrument_id}/{data_type_dir}/YYYY-MM.parquet`, and deletes
    /// the archived `.rara` files.
    ///
    /// Returns the number of files archived.
    pub fn run_for_instrument(
        &self,
        instrument_id: &str,
        data_type_dir: &str,
    ) -> Result<usize> {
        let hot_dir = self.data_dir.join("hot").join(instrument_id).join(data_type_dir);

        if !hot_dir.exists() {
            return Ok(0);
        }

        let today = Zoned::now().date();

        // Collect eligible .rara files grouped by month
        let grouped = self.collect_and_group(&hot_dir, today)?;

        let mut total_archived = 0usize;

        for (month_key, file_paths) in &grouped {
            let records = Self::read_all_candles(file_paths)?;

            let cold_dir = self
                .data_dir
                .join("cold")
                .join(instrument_id)
                .join(data_type_dir);
            fs::create_dir_all(&cold_dir).context(IoSnafu)?;

            let parquet_path = cold_dir.join(format!("{month_key}.parquet"));
            write_candles_to_parquet(&records, &parquet_path)?;

            // Delete archived originals
            for path in file_paths {
                fs::remove_file(path).context(IoSnafu)?;
            }

            total_archived += file_paths.len();
        }

        Ok(total_archived)
    }

    /// Scan the hot directory for `.rara` files older than retention, grouped by month.
    fn collect_and_group(
        &self,
        hot_dir: &std::path::Path,
        today: Date,
    ) -> Result<BTreeMap<String, Vec<PathBuf>>> {
        let entries = fs::read_dir(hot_dir).context(IoSnafu)?;

        let mut grouped: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();

        for entry in entries {
            let entry = entry.context(IoSnafu)?;
            let path = entry.path();

            let ext = path.extension().and_then(|e| e.to_str());
            if ext != Some("rara") {
                continue;
            }

            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };

            // Parse date from filename stem (YYYY-MM-DD)
            let Ok(file_date) = stem.parse::<Date>() else {
                continue;
            };

            // Check age against retention_days
            let age_days = (today - file_date).get_days();
            // retention_days is capped well below i32::MAX in practice
            #[allow(clippy::cast_possible_wrap)]
            if age_days < self.retention_days as i32 {
                continue;
            }

            let month_key = format!(
                "{:04}-{:02}",
                file_date.year(),
                file_date.month().cast_unsigned()
            );
            grouped.entry(month_key).or_default().push(path);
        }

        Ok(grouped)
    }

    /// Read candle records from all given `.rara` files, concatenated in order.
    fn read_all_candles(paths: &[PathBuf]) -> Result<Vec<CandleRecord>> {
        let mut all_records = Vec::new();
        for path in paths {
            let reader = RaraFileReader::open(path).context(FileSnafu)?;
            let records = reader.candle_records().context(FileSnafu)?;
            all_records.extend_from_slice(records);
        }
        Ok(all_records)
    }
}

// ---------------------------------------------------------------------------
// Parquet writing
// ---------------------------------------------------------------------------

/// Write a slice of candle records to a Parquet file at `path`.
fn write_candles_to_parquet(records: &[CandleRecord], path: &std::path::Path) -> Result<()> {
    let schema = Arc::new(Schema::new(vec![
        Field::new("ts_event", ArrowDataType::Int64, false),
        Field::new("open", ArrowDataType::Int64, false),
        Field::new("high", ArrowDataType::Int64, false),
        Field::new("low", ArrowDataType::Int64, false),
        Field::new("close", ArrowDataType::Int64, false),
        Field::new("volume", ArrowDataType::Int64, false),
        Field::new("trade_count", ArrowDataType::UInt32, false),
    ]));

    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from_iter_values(records.iter().map(|r| r.ts_event))),
            Arc::new(Int64Array::from_iter_values(records.iter().map(|r| r.open))),
            Arc::new(Int64Array::from_iter_values(records.iter().map(|r| r.high))),
            Arc::new(Int64Array::from_iter_values(records.iter().map(|r| r.low))),
            Arc::new(Int64Array::from_iter_values(records.iter().map(|r| r.close))),
            Arc::new(Int64Array::from_iter_values(records.iter().map(|r| r.volume))),
            Arc::new(UInt32Array::from_iter_values(
                records.iter().map(|r| r.trade_count),
            )),
        ],
    )
    .map_err(|e| ArchiverError::Arrow {
        message: e.to_string(),
    })?;

    let file = fs::File::create(path).context(IoSnafu)?;
    let mut writer = ArrowWriter::try_new(file, schema, None).map_err(|e| {
        ArchiverError::Parquet {
            message: e.to_string(),
        }
    })?;
    writer.write(&batch).map_err(|e| ArchiverError::Parquet {
        message: e.to_string(),
    })?;
    writer.close().map_err(|e| ArchiverError::Parquet {
        message: e.to_string(),
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::RaraFileWriter;
    use crate::record::{CandleRecord, RecordType};

    fn sample_candle(ts: i64) -> CandleRecord {
        CandleRecord {
            ts_event: ts,
            open: 42_000_000_000_000,
            high: 43_000_000_000_000,
            low: 41_000_000_000_000,
            close: 42_500_000_000_000,
            volume: 100_000_000_000,
            trade_count: 10,
            pad: [0; 12],
        }
    }

    #[test]
    fn archive_old_files_to_parquet() {
        let dir = tempfile::tempdir().unwrap();

        let hot_dir = dir.path().join("hot/test-btc_usdt/candles_1m");
        fs::create_dir_all(&hot_dir).unwrap();

        // Create 3 .rara files for Jan 2026 dates
        for day in 1..=3u8 {
            let file_path = hot_dir.join(format!("2026-01-{day:02}.rara"));
            let mut writer =
                RaraFileWriter::create(&file_path, "test-btc_usdt", RecordType::Candle).unwrap();
            writer
                .append_candles(&[sample_candle(i64::from(day) * 1_000_000_000)])
                .unwrap();
            writer.flush().unwrap();
        }

        let archiver = Archiver::builder()
            .data_dir(dir.path().to_path_buf())
            .retention_days(0) // archive everything
            .build();

        let archived = archiver
            .run_for_instrument("test-btc_usdt", "candles_1m")
            .unwrap();
        assert_eq!(archived, 3);

        // Verify parquet file exists
        let parquet_path = dir
            .path()
            .join("cold/test-btc_usdt/candles_1m/2026-01.parquet");
        assert!(parquet_path.exists());

        // Verify original .rara files deleted
        for day in 1..=3u8 {
            let rara_path = hot_dir.join(format!("2026-01-{day:02}.rara"));
            assert!(!rara_path.exists());
        }
    }
}
