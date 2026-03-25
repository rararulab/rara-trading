//! Binary file reader and writer for the `.rara` market data format.
//!
//! The writer serializes records via [`zerocopy::IntoBytes`] and the reader
//! provides zero-copy access through memory-mapped I/O ([`memmap2::Mmap`]).

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::mem::size_of;
use std::path::Path;

use memmap2::Mmap;
use snafu::{ensure, ResultExt, Snafu};
use zerocopy::{FromBytes, Immutable, IntoBytes};

use crate::record::{CandleRecord, FileHeader, RecordType, TickRecord, MAGIC};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

/// Errors produced by `.rara` file I/O operations.
#[derive(Debug, Snafu)]
#[snafu(visibility(pub))]
pub enum FileError {
    /// An I/O error occurred while accessing the file at `path`.
    #[snafu(display("I/O error on {path}: {source}"))]
    Io {
        /// Path to the file that triggered the error.
        path: String,
        /// Underlying OS error.
        source: std::io::Error,
    },

    /// The file does not start with the expected `RARA` magic bytes.
    #[snafu(display("invalid magic bytes in {path}"))]
    InvalidMagic {
        /// Path to the file with bad magic.
        path: String,
    },

    /// The file is too small to contain a valid header.
    #[snafu(display("file too small: {path}"))]
    TooSmall {
        /// Path to the undersized file.
        path: String,
    },

    /// The record type stored in the file does not match the requested type.
    #[snafu(display("type mismatch: file has {file_type}, requested {requested}"))]
    TypeMismatch {
        /// Record type discriminant stored in the file header.
        file_type: u16,
        /// Record type discriminant the caller requested.
        requested: u16,
    },

    /// The data region is not properly aligned for the record type.
    #[snafu(display("unaligned data in {path}"))]
    UnalignedData {
        /// Path to the file with alignment issues.
        path: String,
    },
}

/// Convenience alias for file operations.
pub type Result<T> = std::result::Result<T, FileError>;

// ---------------------------------------------------------------------------
// RaraFile helper
// ---------------------------------------------------------------------------

/// Utility for constructing a [`FileHeader`].
pub struct RaraFile;

impl RaraFile {
    /// Build a [`FileHeader`] for the given instrument and record type.
    pub fn make_header(instrument_id: &str, record_type: RecordType) -> FileHeader {
        let mut id_buf = [0u8; 32];
        let len = instrument_id.len().min(32);
        id_buf[..len].copy_from_slice(&instrument_id.as_bytes()[..len]);

        FileHeader {
            magic: MAGIC,
            version: 1,
            record_type: record_type as u16,
            // record_size is always 32 or 64, safe to truncate
            #[allow(clippy::cast_possible_truncation)]
            record_size: record_type.record_size() as u32,
            record_count: 0,
            instrument_id: id_buf,
            reserved: [0u8; 16],
        }
    }
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Writes `.rara` binary files, supporting both creation and incremental append.
pub struct RaraFileWriter {
    file: File,
    header: FileHeader,
    path: String,
}

impl RaraFileWriter {
    /// Create a new `.rara` file at `path`, overwriting any existing file.
    pub fn create(path: &Path, instrument_id: &str, record_type: RecordType) -> Result<Self> {
        let path_str = path.display().to_string();
        let mut file = File::create(path).context(IoSnafu { path: &path_str })?;
        let header = RaraFile::make_header(instrument_id, record_type);
        file.write_all(header.as_bytes())
            .context(IoSnafu { path: &path_str })?;
        Ok(Self {
            file,
            header,
            path: path_str,
        })
    }

    /// Open an existing `.rara` file for appending additional records.
    pub fn open_append(path: &Path) -> Result<Self> {
        let path_str = path.display().to_string();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .context(IoSnafu { path: &path_str })?;

        let mut header_bytes = [0u8; size_of::<FileHeader>()];
        file.read_exact(&mut header_bytes)
            .context(IoSnafu { path: &path_str })?;

        let header =
            FileHeader::read_from_bytes(&header_bytes).map_err(|_| FileError::TooSmall {
                path: path_str.clone(),
            })?;

        ensure!(
            header.magic == MAGIC,
            InvalidMagicSnafu { path: &path_str }
        );

        // Seek to end for appending
        file.seek(SeekFrom::End(0))
            .context(IoSnafu { path: &path_str })?;

        Ok(Self {
            file,
            header,
            path: path_str,
        })
    }

    /// Append candle records to the file.
    pub fn append_candles(&mut self, records: &[CandleRecord]) -> Result<()> {
        ensure!(
            self.header.record_type == RecordType::Candle as u16,
            TypeMismatchSnafu {
                file_type: self.header.record_type,
                requested: RecordType::Candle as u16,
            }
        );
        for rec in records {
            self.file
                .write_all(rec.as_bytes())
                .context(IoSnafu { path: &self.path })?;
        }
        // Record slices are bounded by available memory; truncation is safe in practice.
        #[allow(clippy::cast_possible_truncation)]
        {
            self.header.record_count += records.len() as u32;
        }
        Ok(())
    }

    /// Append tick records to the file.
    pub fn append_ticks(&mut self, records: &[TickRecord]) -> Result<()> {
        ensure!(
            self.header.record_type == RecordType::Tick as u16,
            TypeMismatchSnafu {
                file_type: self.header.record_type,
                requested: RecordType::Tick as u16,
            }
        );
        for rec in records {
            self.file
                .write_all(rec.as_bytes())
                .context(IoSnafu { path: &self.path })?;
        }
        #[allow(clippy::cast_possible_truncation)]
        {
            self.header.record_count += records.len() as u32;
        }
        Ok(())
    }

    /// Flush pending writes and update the header's `record_count` at offset 0.
    pub fn flush(&mut self) -> Result<()> {
        self.file
            .flush()
            .context(IoSnafu { path: &self.path })?;
        self.file
            .seek(SeekFrom::Start(0))
            .context(IoSnafu { path: &self.path })?;
        self.file
            .write_all(self.header.as_bytes())
            .context(IoSnafu { path: &self.path })?;
        self.file
            .flush()
            .context(IoSnafu { path: &self.path })?;
        // Seek back to end so subsequent appends work
        self.file
            .seek(SeekFrom::End(0))
            .context(IoSnafu { path: &self.path })?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// Zero-copy reader for `.rara` files backed by memory-mapped I/O.
#[derive(Debug)]
pub struct RaraFileReader {
    mmap: Mmap,
    header: FileHeader,
    path: String,
}

impl RaraFileReader {
    /// Open a `.rara` file for reading via mmap.
    pub fn open(path: &Path) -> Result<Self> {
        let path_str = path.display().to_string();
        let file = File::open(path).context(IoSnafu { path: &path_str })?;

        let metadata = file
            .metadata()
            .context(IoSnafu { path: &path_str })?;
        ensure!(
            metadata.len() >= size_of::<FileHeader>() as u64,
            TooSmallSnafu { path: &path_str }
        );

        // SAFETY: The file is opened read-only; concurrent modification is
        // not expected in our single-process pipeline. The mmap lifetime is
        // bounded by `self`.
        #[allow(unsafe_code)]
        let mmap = unsafe { Mmap::map(&file) }.context(IoSnafu { path: &path_str })?;

        let (header, _) = FileHeader::read_from_prefix(&mmap[..]).map_err(|_| {
            FileError::TooSmall {
                path: path_str.clone(),
            }
        })?;

        ensure!(
            header.magic == MAGIC,
            InvalidMagicSnafu { path: &path_str }
        );

        Ok(Self {
            mmap,
            header,
            path: path_str,
        })
    }

    /// Return the file header.
    pub const fn header(&self) -> &FileHeader {
        &self.header
    }

    /// Return the data region as a zero-copy slice of [`CandleRecord`].
    pub fn candle_records(&self) -> Result<&[CandleRecord]> {
        ensure!(
            self.header.record_type == RecordType::Candle as u16,
            TypeMismatchSnafu {
                file_type: self.header.record_type,
                requested: RecordType::Candle as u16,
            }
        );
        self.records_slice()
    }

    /// Return the data region as a zero-copy slice of [`TickRecord`].
    pub fn tick_records(&self) -> Result<&[TickRecord]> {
        ensure!(
            self.header.record_type == RecordType::Tick as u16,
            TypeMismatchSnafu {
                file_type: self.header.record_type,
                requested: RecordType::Tick as u16,
            }
        );
        self.records_slice()
    }

    /// Return the timestamp of the last candle, if any.
    pub fn last_candle_timestamp(&self) -> Result<Option<i64>> {
        let records = self.candle_records()?;
        Ok(records.last().map(|r| r.ts_event))
    }

    /// Return the timestamp of the last tick, if any.
    pub fn last_tick_timestamp(&self) -> Result<Option<i64>> {
        let records = self.tick_records()?;
        Ok(records.last().map(|r| r.ts_event))
    }

    /// Generic helper to cast the data region into a typed slice.
    fn records_slice<T: FromBytes + Immutable>(&self) -> Result<&[T]> {
        let count = self.header.record_count as usize;
        if count == 0 {
            return Ok(&[]);
        }

        let data = &self.mmap[size_of::<FileHeader>()..];

        let (records, _) =
            <[T]>::ref_from_prefix_with_elems(data, count).map_err(|_| {
                FileError::UnalignedData {
                    path: self.path.clone(),
                }
            })?;

        Ok(records)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

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
    fn write_and_read_candles() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.rara");

        let candles = [sample_candle(1000), sample_candle(2000)];
        {
            let mut w = RaraFileWriter::create(&path, "BTC-USDT", RecordType::Candle)
                .expect("create");
            w.append_candles(&candles).expect("append");
            w.flush().expect("flush");
        }

        let reader = RaraFileReader::open(&path).expect("open");
        assert_eq!(reader.header().record_count, 2);
        let read_candles = reader.candle_records().expect("candle_records");
        assert_eq!(read_candles.len(), 2);
        assert_eq!(read_candles[0], candles[0]);
        assert_eq!(read_candles[1], candles[1]);
    }

    #[test]
    fn append_incremental() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.rara");

        {
            let mut w = RaraFileWriter::create(&path, "ETH-USDT", RecordType::Candle)
                .expect("create");
            w.append_candles(&[sample_candle(100)]).expect("append");
            w.flush().expect("flush");
        }

        {
            let mut w = RaraFileWriter::open_append(&path).expect("open_append");
            w.append_candles(&[sample_candle(200)]).expect("append");
            w.flush().expect("flush");
        }

        let reader = RaraFileReader::open(&path).expect("open");
        assert_eq!(reader.header().record_count, 2);
        let records = reader.candle_records().expect("candle_records");
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].ts_event, 100);
        assert_eq!(records[1].ts_event, 200);
    }

    #[test]
    fn last_timestamp_for_dedup() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.rara");

        {
            let mut w = RaraFileWriter::create(&path, "BTC-USDT", RecordType::Candle)
                .expect("create");
            w.append_candles(&[sample_candle(1000), sample_candle(2000)])
                .expect("append");
            w.flush().expect("flush");
        }

        let reader = RaraFileReader::open(&path).expect("open");
        assert_eq!(reader.last_candle_timestamp().expect("ts"), Some(2000));
    }

    #[test]
    fn empty_file_returns_empty_slice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.rara");

        {
            let mut w = RaraFileWriter::create(&path, "BTC-USDT", RecordType::Candle)
                .expect("create");
            w.flush().expect("flush");
        }

        let reader = RaraFileReader::open(&path).expect("open");
        assert_eq!(reader.header().record_count, 0);
        let records = reader.candle_records().expect("candle_records");
        assert!(records.is_empty());
    }

    #[test]
    fn reject_bad_magic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("bad.rara");

        std::fs::write(&path, [0u8; 64]).expect("write zeros");

        let err = RaraFileReader::open(&path).unwrap_err();
        assert!(
            matches!(err, FileError::InvalidMagic { .. }),
            "expected InvalidMagic, got: {err:?}"
        );
    }
}
