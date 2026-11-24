use std::collections::HashMap;
use std::sync::{Arc, PoisonError, RwLock};

/// The stream token of each webview, published by the hub thread (which owns
/// issuing and revoking them) so request handlers can reject a missing or
/// stale token without a round trip to it.
#[derive(Default)]
pub struct StreamTokens {
    by_label: RwLock<HashMap<Arc<str>, u64>>,
}

impl StreamTokens {
    pub fn publish(&self, label: &Arc<str>, token: u64) {
        self.by_label
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(label.clone(), token);
    }

    pub fn revoke(&self, label: &str) {
        self.by_label
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(label);
    }

    pub fn authorizes(&self, label: &str, token: u64) -> bool {
        self.by_label
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(label)
            .is_some_and(|issued| ct_eq(*issued, token))
    }
}

/// Branch-free equality, so the comparison time does not depend on how many
/// leading bits of a guess are right.
fn ct_eq(a: u64, b: u64) -> bool {
    let diff = std::hint::black_box(a ^ b);
    (diff | diff.wrapping_neg()) >> 63 == 0
}
