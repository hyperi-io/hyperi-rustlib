// Project:   hs-rustlib
// File:      src/tiered_sink/codec.rs
// Purpose:   Compression codec selection for spool storage
// Language:  Rust
//
// License:   LicenseRef-HyperSec-EULA
// Copyright: (c) 2025 HyperSec

//! Compression codec selection for spool storage.

use serde::{Deserialize, Serialize};
use std::io;

/// Compression codec for spool storage.
///
/// Different codecs offer different CPU/compression tradeoffs:
/// - `Lz4`: Fast compression, low CPU - best for hot-path fallback (default)
/// - `Snappy`: Very fast, Kafka-native - avoids transcode if sink uses Snappy
/// - `Zstd`: Best compression ratio, higher CPU - good for constrained disk
/// - `None`: No compression - maximum speed when CPU is bottleneck
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompressionCodec {
    /// No compression - fastest, no CPU overhead
    None,
    /// LZ4 - fast compression, low CPU (default)
    ///
    /// LZ4 is the best default for hot-path fallback:
    /// - Very low CPU overhead
    /// - Still provides meaningful compression
    /// - Pure Rust implementation (lz4_flex)
    #[default]
    Lz4,
    /// Snappy - very fast, Kafka-native format
    Snappy,
    /// Zstd with configurable level (1-22)
    Zstd { level: i32 },
}

impl CompressionCodec {
    /// Create Zstd codec with default level (3).
    #[must_use]
    pub fn zstd() -> Self {
        Self::Zstd { level: 3 }
    }

    /// Create Zstd codec with specified level.
    #[must_use]
    pub fn zstd_level(level: i32) -> Self {
        Self::Zstd {
            level: level.clamp(1, 22),
        }
    }

    /// Compress data using this codec.
    ///
    /// # Errors
    ///
    /// Returns an error if compression fails.
    pub fn compress(&self, data: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Self::None => Ok(data.to_vec()),
            Self::Lz4 => Ok(lz4_flex::compress_prepend_size(data)),
            Self::Snappy => {
                let mut encoder = snap::raw::Encoder::new();
                encoder
                    .compress_vec(data)
                    .map_err(io::Error::other)
            }
            Self::Zstd { level } => {
                zstd::encode_all(data, *level).map_err(io::Error::other)
            }
        }
    }

    /// Decompress data using this codec.
    ///
    /// # Errors
    ///
    /// Returns an error if decompression fails.
    pub fn decompress(&self, data: &[u8]) -> io::Result<Vec<u8>> {
        match self {
            Self::None => Ok(data.to_vec()),
            Self::Lz4 => lz4_flex::decompress_size_prepended(data)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string())),
            Self::Snappy => {
                let mut decoder = snap::raw::Decoder::new();
                decoder
                    .decompress_vec(data)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
            }
            Self::Zstd { .. } => {
                zstd::decode_all(data)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
            }
        }
    }

    /// Returns true if this codec applies compression.
    #[must_use]
    pub fn is_compressed(&self) -> bool {
        !matches!(self, Self::None)
    }
}

impl std::fmt::Display for CompressionCodec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "none"),
            Self::Lz4 => write!(f, "lz4"),
            Self::Snappy => write!(f, "snappy"),
            Self::Zstd { level } => write!(f, "zstd(level={level})"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_is_lz4() {
        assert_eq!(CompressionCodec::default(), CompressionCodec::Lz4);
    }

    #[test]
    fn test_none_roundtrip() {
        let codec = CompressionCodec::None;
        let data = b"hello world";
        let compressed = codec.compress(data).unwrap();
        let decompressed = codec.decompress(&compressed).unwrap();
        assert_eq!(data.as_slice(), decompressed.as_slice());
        assert_eq!(compressed, data); // No change
    }

    #[test]
    fn test_lz4_roundtrip() {
        let codec = CompressionCodec::Lz4;
        let data = b"hello world hello world hello world";
        let compressed = codec.compress(data).unwrap();
        let decompressed = codec.decompress(&compressed).unwrap();
        assert_eq!(data.as_slice(), decompressed.as_slice());
        assert!(compressed.len() < data.len()); // Actually compressed
    }

    #[test]
    fn test_snappy_roundtrip() {
        let codec = CompressionCodec::Snappy;
        let data = b"hello world hello world hello world";
        let compressed = codec.compress(data).unwrap();
        let decompressed = codec.decompress(&compressed).unwrap();
        assert_eq!(data.as_slice(), decompressed.as_slice());
    }

    #[test]
    fn test_zstd_roundtrip() {
        let codec = CompressionCodec::zstd();
        let data = b"hello world hello world hello world";
        let compressed = codec.compress(data).unwrap();
        let decompressed = codec.decompress(&compressed).unwrap();
        assert_eq!(data.as_slice(), decompressed.as_slice());
        assert!(compressed.len() < data.len());
    }

    #[test]
    fn test_zstd_level_clamped() {
        let codec = CompressionCodec::zstd_level(100);
        assert!(matches!(codec, CompressionCodec::Zstd { level: 22 }));

        let codec = CompressionCodec::zstd_level(-5);
        assert!(matches!(codec, CompressionCodec::Zstd { level: 1 }));
    }

    #[test]
    fn test_is_compressed() {
        assert!(!CompressionCodec::None.is_compressed());
        assert!(CompressionCodec::Lz4.is_compressed());
        assert!(CompressionCodec::Snappy.is_compressed());
        assert!(CompressionCodec::zstd().is_compressed());
    }

    #[test]
    fn test_display() {
        assert_eq!(format!("{}", CompressionCodec::None), "none");
        assert_eq!(format!("{}", CompressionCodec::Lz4), "lz4");
        assert_eq!(format!("{}", CompressionCodec::Snappy), "snappy");
        assert_eq!(format!("{}", CompressionCodec::zstd()), "zstd(level=3)");
    }
}
