//! Moka-backed concurrent LRU cache for mmap'd market data files.

use std::path::PathBuf;
use std::sync::Arc;

use moka::sync::Cache;
use snafu::{ResultExt, Snafu};

use crate::file::{FileError, RaraFileReader};
use crate::record::{CandleRecord, TickRecord};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors produced by the data cache.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum CacheError {
    /// Failed to load a `.rara` file from disk.
    #[snafu(display("failed to load {path}: {source}"))]
    LoadFile {
        /// Path that failed to load.
        path: String,
        /// Underlying file error.
        source: FileError,
    },
}

/// Convenience alias for cache operations.
pub type Result<T> = std::result::Result<T, CacheError>;

// ---------------------------------------------------------------------------
// DataType
// ---------------------------------------------------------------------------

/// Discriminant for the kind of market data stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    /// 1-minute OHLCV candles.
    Candle1m,
    /// 5-minute OHLCV candles.
    Candle5m,
    /// Individual trade ticks.
    Ticks,
}

impl DataType {
    /// Return the directory name used for on-disk storage.
    pub const fn dir_name(self) -> &'static str {
        match self {
            Self::Candle1m => "candles_1m",
            Self::Candle5m => "candles_5m",
            Self::Ticks => "ticks",
        }
    }
}

// ---------------------------------------------------------------------------
// DataKey
// ---------------------------------------------------------------------------

/// Cache key identifying a single date-file of market data.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DataKey {
    /// Exchange instrument identifier (e.g. `"BTC-USDT"`).
    pub instrument_id: String,
    /// Kind of market data.
    pub data_type: DataType,
    /// Date string in `"YYYY-MM-DD"` format.
    pub date: String,
}

// ---------------------------------------------------------------------------
// MarketSlice
// ---------------------------------------------------------------------------

/// A cached slice of market data backed by a memory-mapped `.rara` file.
pub struct MarketSlice {
    reader: RaraFileReader,
}

impl MarketSlice {
    /// Number of candle records in this slice.
    pub fn candle_count(&self) -> crate::file::Result<usize> {
        self.reader.candle_records().map(<[CandleRecord]>::len)
    }

    /// Number of tick records in this slice.
    pub fn tick_count(&self) -> crate::file::Result<usize> {
        self.reader.tick_records().map(<[TickRecord]>::len)
    }

    /// Return the candle records as a zero-copy slice.
    pub fn candles(&self) -> crate::file::Result<&[CandleRecord]> {
        self.reader.candle_records()
    }

    /// Return the tick records as a zero-copy slice.
    pub fn ticks(&self) -> crate::file::Result<&[TickRecord]> {
        self.reader.tick_records()
    }

    /// Total byte length of the underlying memory-mapped region.
    pub const fn byte_len(&self) -> usize {
        // header record_count * record_size + header size
        let h = self.reader.header();
        std::mem::size_of::<crate::record::FileHeader>()
            + (h.record_count as usize) * (h.record_size as usize)
    }
}

// ---------------------------------------------------------------------------
// CacheStats
// ---------------------------------------------------------------------------

/// Snapshot of cache utilization metrics.
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// Number of entries currently in the cache.
    pub entry_count: u64,
    /// Approximate weighted size in bytes.
    pub weighted_size: u64,
}

// ---------------------------------------------------------------------------
// DataCache
// ---------------------------------------------------------------------------

/// Concurrent LRU cache for memory-mapped market data files.
///
/// Uses moka's weighted eviction so the total cached byte footprint stays
/// within `max_capacity_bytes`.
#[derive(Clone)]
pub struct DataCache {
    data_dir: PathBuf,
    max_capacity_bytes: u64,
    cache: Cache<DataKey, Arc<MarketSlice>>,
}

impl DataCache {
    /// Create a new `DataCache`.
    ///
    /// `data_dir` is the root directory containing `hot/{instrument_id}/{data_type}/{date}.rara`.
    /// `max_capacity_bytes` caps the total weighted size of cached entries.
    pub fn new(data_dir: PathBuf, max_capacity_bytes: u64) -> Self {
        let cache = Cache::builder()
            .weigher(|_key: &DataKey, val: &Arc<MarketSlice>| -> u32 {
                // Clamp to u32::MAX for moka weigher contract
                #[allow(clippy::cast_possible_truncation)]
                // Value is clamped to u32::MAX before truncation
                { val.byte_len().min(u32::MAX as usize) as u32 }
            })
            .max_capacity(max_capacity_bytes)
            .build();

        Self {
            data_dir,
            max_capacity_bytes,
            cache,
        }
    }

    /// Get a market slice by key, loading from disk on cache miss.
    pub fn get(&self, key: &DataKey) -> Result<Arc<MarketSlice>> {
        let data_dir = self.data_dir.clone();
        let key_owned = key.clone();

        self.cache
            .try_get_with(key.clone(), move || {
                let path = data_dir
                    .join("hot")
                    .join(&key_owned.instrument_id)
                    .join(key_owned.data_type.dir_name())
                    .join(format!("{}.rara", key_owned.date));

                let path_str = path.display().to_string();
                let reader =
                    RaraFileReader::open(&path).context(LoadFileSnafu { path: path_str })?;
                Ok(Arc::new(MarketSlice { reader }))
            })
            .map_err(|e: Arc<CacheError>| {
                // Unwrap the Arc — clone the inner error's display for a new instance.
                // Since CacheError variants are clonable data, reconstruct from the Arc.
                match e.as_ref() {
                    CacheError::LoadFile { path, source } => CacheError::LoadFile {
                        path: path.clone(),
                        source: clone_file_error(source),
                    },
                }
            })
    }

    /// Evict a specific entry from the cache.
    pub fn invalidate(&self, instrument_id: &str, data_type: DataType, date: &str) {
        let key = DataKey {
            instrument_id: instrument_id.to_string(),
            data_type,
            date: date.to_string(),
        };
        self.cache.invalidate(&key);
    }

    /// Load market slices for a contiguous date range, skipping missing files.
    pub fn load_range(
        &self,
        instrument_id: &str,
        data_type: DataType,
        start_date: &str,
        end_date: &str,
    ) -> Result<Vec<Arc<MarketSlice>>> {
        enumerate_dates(start_date, end_date)
            .into_iter()
            .filter_map(|date| {
                let key = DataKey {
                    instrument_id: instrument_id.to_string(),
                    data_type,
                    date,
                };
                match self.get(&key) {
                    Ok(slice) => Some(Ok(slice)),
                    Err(CacheError::LoadFile { ref source, .. })
                        if matches!(source, FileError::Io { .. }) =>
                    {
                        // File not found — skip this date
                        None
                    }
                    Err(e) => Some(Err(e)),
                }
            })
            .collect()
    }

    /// Return a snapshot of cache statistics.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entry_count: self.cache.entry_count(),
            weighted_size: self.cache.weighted_size(),
        }
    }

    /// Return the configured maximum capacity in bytes.
    pub const fn max_capacity_bytes(&self) -> u64 {
        self.max_capacity_bytes
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Enumerate calendar dates from `start` to `end` (inclusive) as `"YYYY-MM-DD"` strings.
fn enumerate_dates(start: &str, end: &str) -> Vec<String> {
    let Ok(start_date) = start.parse::<jiff::civil::Date>() else {
        return vec![];
    };
    let Ok(end_date) = end.parse::<jiff::civil::Date>() else {
        return vec![];
    };

    let mut dates = Vec::new();
    let mut current = start_date;
    while current <= end_date {
        dates.push(current.to_string());
        // Advance by one day
        current = current.tomorrow().unwrap_or(current);
        if current == start_date {
            // tomorrow() failed (end of representable range)
            break;
        }
    }
    dates
}

/// Clone a `FileError` for re-wrapping after moka's `Arc<E>` return.
fn clone_file_error(e: &FileError) -> FileError {
    match e {
        FileError::Io { path, source } => FileError::Io {
            path: path.clone(),
            source: std::io::Error::new(source.kind(), source.to_string()),
        },
        FileError::InvalidMagic { path } => FileError::InvalidMagic { path: path.clone() },
        FileError::TooSmall { path } => FileError::TooSmall { path: path.clone() },
        FileError::TypeMismatch {
            file_type,
            requested,
        } => FileError::TypeMismatch {
            file_type: *file_type,
            requested: *requested,
        },
        FileError::UnalignedData { path } => FileError::UnalignedData { path: path.clone() },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::{DataCache, DataType};
    use crate::file::RaraFileWriter;
    use crate::record::{CandleRecord, RecordType};

    /// Create a test `.rara` candle file with `n` records at the conventional path.
    fn create_test_candle_file(dir: &TempDir, instrument_id: &str, date: &str, n: usize) {
        let path = dir
            .path()
            .join("hot")
            .join(instrument_id)
            .join("candles_1m")
            .join(format!("{date}.rara"));
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");

        let records: Vec<CandleRecord> = (0..n)
            .map(|i| CandleRecord {
                #[allow(clippy::cast_possible_wrap)]
                ts_event: (i as i64) * 60_000_000_000,
                open: 42_000_000_000_000,
                high: 43_000_000_000_000,
                low: 41_000_000_000_000,
                close: 42_500_000_000_000,
                volume: 100_000_000_000,
                trade_count: 10,
                pad: [0; 12],
            })
            .collect();

        let mut writer =
            RaraFileWriter::create(&path, instrument_id, RecordType::Candle).expect("create");
        writer.append_candles(&records).expect("append");
        writer.flush().expect("flush");
    }

    #[test]
    fn load_range_returns_ordered_slices() {
        let dir = tempfile::tempdir().expect("tempdir");
        create_test_candle_file(&dir, "ETH-USDT", "2024-03-01", 3);
        create_test_candle_file(&dir, "ETH-USDT", "2024-03-02", 7);

        let cache = DataCache::new(dir.path().to_path_buf(), 100 * 1024 * 1024);
        let slices = cache
            .load_range("ETH-USDT", DataType::Candle1m, "2024-03-01", "2024-03-02")
            .expect("load_range");

        assert_eq!(slices.len(), 2);
        assert_eq!(slices[0].candle_count().expect("count"), 3);
        assert_eq!(slices[1].candle_count().expect("count"), 7);
    }

    #[test]
    fn missing_date_in_range_skipped() {
        let dir = tempfile::tempdir().expect("tempdir");
        // Only create the first date file, not the second
        create_test_candle_file(&dir, "SOL-USDT", "2024-06-10", 4);

        let cache = DataCache::new(dir.path().to_path_buf(), 100 * 1024 * 1024);
        let slices = cache
            .load_range("SOL-USDT", DataType::Candle1m, "2024-06-10", "2024-06-11")
            .expect("load_range");

        assert_eq!(slices.len(), 1, "missing date should be skipped");
        assert_eq!(slices[0].candle_count().expect("count"), 4);
    }
}
