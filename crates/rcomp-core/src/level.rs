//! Unified compression level abstraction.
//!
//! The three levels describe the **output ratio**, never the hardware cost.
//! Codec-native multithreading is used whenever available at every level
//! because it does not change the compressed output.
//!
//! | Level  | Intent                                                         |
//! |--------|----------------------------------------------------------------|
//! | `Fast` | Lowest latency; ratio traded away for speed.                   |
//! | `Best` | Balanced default — reasonable ratio at reasonable cost.        |
//! | `Edge` | Maximum output ratio: every ratio-improving codec feature on.  |
//!
//! The concrete codec parameters that correspond to each level are defined in
//! the individual codec backends (milestone 2).

/// Unified compression level.
///
/// `Best` is the default — it gives a balanced tradeoff between output size
/// and CPU time. `Edge` activates every ratio-improving option a codec offers
/// (e.g. zstd level 22 + long-distance matching, xz level 9 + `--extreme`),
/// without regard for hardware cost. `Fast` favours lowest latency over ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Level {
    /// Lowest latency; output ratio is traded away for speed.
    Fast,
    /// Balanced default — reasonable ratio, reasonable CPU cost.
    #[default]
    Best,
    /// Maximum output ratio; activates every ratio-improving codec feature.
    Edge,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_best() {
        assert_eq!(Level::default(), Level::Best);
    }

    #[test]
    fn variants_are_distinct() {
        assert_ne!(Level::Fast, Level::Best);
        assert_ne!(Level::Best, Level::Edge);
        assert_ne!(Level::Fast, Level::Edge);
    }

    #[test]
    #[allow(clippy::clone_on_copy)]
    fn clone_and_copy() {
        let a = Level::Edge;
        let b = a; // Copy
        let c = a.clone(); // Clone
        assert_eq!(a, b);
        assert_eq!(a, c);
    }
}
