//! Penjaga "sedang diproses": satu kunci (mis. id usulan atau tombol) hanya boleh dikerjakan
//! satu permintaan sekaligus. Mencegah ketukan dobel atau dua tab menjalankan hal yang sama.

use std::collections::HashSet;
use std::sync::Mutex;

static BUSY: Mutex<Option<HashSet<String>>> = Mutex::new(None);

pub struct Claim(String);

/// `None` bila kunci yang sama sedang diproses permintaan lain.
pub fn claim(key: impl Into<String>) -> Option<Claim> {
    let key = key.into();
    let fresh = BUSY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(HashSet::new)
        .insert(key.clone());
    // Claim dibuat hanya bila kunci baru masuk; Drop-nya mengunci BUSY lagi.
    fresh.then(|| Claim(key))
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(set) = BUSY.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            set.remove(&self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn second_claim_waits_for_first() {
        let a = super::claim("tes-x").unwrap();
        assert!(super::claim("tes-x").is_none());
        drop(a);
        assert!(super::claim("tes-x").is_some());
    }
}
