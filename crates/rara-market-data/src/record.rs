//! Fixed-width binary record types for zero-copy market data storage.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

/// Scale factor for fixed-point price and quantity representation (10^9).
pub const FIXED_POINT_SCALE: i64 = 1_000_000_000;

/// Magic bytes for `.rara` file identification (`b"RARA"` as little-endian u32).
pub const MAGIC: u32 = u32::from_le_bytes(*b"RARA");

/// Discriminant for the type of records stored in a `.rara` file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum RecordType {
    /// OHLCV candle record (64 bytes each).
    Candle = 1,
    /// Individual trade tick record (32 bytes each).
    Tick = 2,
}

impl RecordType {
    /// Convert a raw `u16` discriminant into a `RecordType`, if valid.
    pub const fn from_u16(v: u16) -> Option<Self> {
        match v {
            1 => Some(Self::Candle),
            2 => Some(Self::Tick),
            _ => None,
        }
    }

    /// Return the byte size of a single record for this type.
    pub const fn record_size(self) -> usize {
        match self {
            Self::Candle => size_of::<CandleRecord>(),
            Self::Tick => size_of::<TickRecord>(),
        }
    }
}

/// 64-byte file header describing the contents of a `.rara` binary file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct FileHeader {
    /// Magic number — must equal [`MAGIC`] (`b"RARA"` as u32).
    pub magic: u32,
    /// Format version (currently `1`).
    pub version: u16,
    /// [`RecordType`] discriminant stored as u16.
    pub record_type: u16,
    /// Size in bytes of each record.
    pub record_size: u32,
    /// Total number of records in the file.
    pub record_count: u32,
    /// Null-padded UTF-8 instrument identifier (e.g. `"BTC-USDT"`).
    pub instrument_id: [u8; 32],
    /// Reserved for future use; must be zeroed.
    pub reserved: [u8; 16],
}

/// 64-byte OHLCV candle record in fixed-point representation.
///
/// Prices and volume are stored as `value × 10^9` integers to avoid
/// floating-point imprecision in financial calculations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct CandleRecord {
    /// Candle open timestamp as nanoseconds since Unix epoch.
    pub ts_event: i64,
    /// Open price scaled by [`FIXED_POINT_SCALE`].
    pub open: i64,
    /// High price scaled by [`FIXED_POINT_SCALE`].
    pub high: i64,
    /// Low price scaled by [`FIXED_POINT_SCALE`].
    pub low: i64,
    /// Close price scaled by [`FIXED_POINT_SCALE`].
    pub close: i64,
    /// Volume scaled by [`FIXED_POINT_SCALE`].
    pub volume: i64,
    /// Number of trades aggregated into this candle.
    pub trade_count: u32,
    /// Padding to align the struct to 64 bytes.
    pub pad: [u8; 12],
}

/// 32-byte individual trade tick record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromBytes, IntoBytes, Immutable, KnownLayout)]
#[repr(C)]
pub struct TickRecord {
    /// Trade timestamp as nanoseconds since Unix epoch.
    pub ts_event: i64,
    /// Trade price scaled by [`FIXED_POINT_SCALE`].
    pub price: i64,
    /// Trade amount scaled by [`FIXED_POINT_SCALE`].
    pub amount: i64,
    /// Trade side: `0` = Buy, `1` = Sell.
    pub side: u8,
    /// Padding to align the struct to 32 bytes.
    pub pad: [u8; 7],
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use zerocopy::{FromBytes, IntoBytes};

    use super::{CandleRecord, FileHeader, TickRecord};

    #[test]
    fn candle_record_is_64_bytes() {
        assert_eq!(size_of::<CandleRecord>(), 64);
    }

    #[test]
    fn tick_record_is_32_bytes() {
        assert_eq!(size_of::<TickRecord>(), 32);
    }

    #[test]
    fn file_header_is_64_bytes() {
        assert_eq!(size_of::<FileHeader>(), 64);
    }

    #[test]
    fn candle_record_roundtrip_bytes() {
        let original = CandleRecord {
            ts_event: 1_700_000_000_000_000_000,
            open: 42_000_000_000_000,
            high: 43_000_000_000_000,
            low: 41_000_000_000_000,
            close: 42_500_000_000_000,
            volume: 100_000_000_000,
            trade_count: 1234,
            pad: [0; 12],
        };

        let bytes = original.as_bytes();
        let parsed = CandleRecord::read_from_bytes(bytes).expect("valid candle bytes");
        assert_eq!(original, parsed);
    }

    #[test]
    fn tick_record_roundtrip_bytes() {
        let original = TickRecord {
            ts_event: 1_700_000_000_000_000_000,
            price: 42_000_000_000_000,
            amount: 1_500_000_000,
            side: 1,
            pad: [0; 7],
        };

        let bytes = original.as_bytes();
        let parsed = TickRecord::read_from_bytes(bytes).expect("valid tick bytes");
        assert_eq!(original, parsed);
    }
}
