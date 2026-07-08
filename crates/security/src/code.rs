use rand::Rng;
use subtle::ConstantTimeEq;

/// Alphabet without visually ambiguous characters (no 0/O, 1/I/L) so a user can
/// read the code aloud without confusion.
const ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTUVWXYZ";
const GROUP_LEN: usize = 4;
const GROUPS: usize = 2;

/// A single-use session code shown on the host and typed on the viewer.
///
/// The value is generated from the OS CSPRNG. It is compared in constant time to
/// avoid leaking how many leading characters matched.
#[derive(Clone)]
pub struct SessionCode(String);

impl SessionCode {
    /// Generates a fresh random code such as `K7QP-2M9X`.
    pub fn generate() -> Self {
        let mut rng = rand::thread_rng();
        let mut groups = Vec::with_capacity(GROUPS);
        for _ in 0..GROUPS {
            let group: String = (0..GROUP_LEN)
                .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
                .collect();
            groups.push(group);
        }
        Self(groups.join("-"))
    }

    /// The human-readable form (with separator) to display on the host.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Checks a candidate against this code in constant time.
    ///
    /// Input is normalized (uppercased, separators removed) so the viewer's user
    /// may type `k7qp2m9x`, `K7QP-2M9X`, etc.
    pub fn verify(&self, candidate: &str) -> bool {
        verify_code(&self.0, candidate)
    }
}

impl std::fmt::Debug for SessionCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print the secret in logs.
        f.write_str("SessionCode(***)")
    }
}

fn normalize(code: &str) -> String {
    code.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Constant-time comparison of two session codes after normalization.
pub fn verify_code(expected: &str, candidate: &str) -> bool {
    let expected = normalize(expected);
    let candidate = normalize(candidate);
    // Length is not secret (fixed by construction); comparing equal-length byte
    // strings in constant time avoids leaking how much of the prefix matched.
    if expected.len() != candidate.len() {
        return false;
    }
    expected.as_bytes().ct_eq(candidate.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_code_has_expected_shape() {
        let code = SessionCode::generate();
        let s = code.as_str();
        assert_eq!(s.len(), GROUPS * GROUP_LEN + (GROUPS - 1)); // groups + separators
        assert!(s.chars().all(|c| c == '-' || ALPHABET.contains(&(c as u8))));
    }

    #[test]
    fn verify_accepts_normalized_variants() {
        let code = SessionCode("K7QP-2M9X".into());
        assert!(code.verify("K7QP-2M9X"));
        assert!(code.verify("k7qp2m9x"));
        assert!(code.verify("k7qp 2m9x"));
    }

    #[test]
    fn verify_rejects_wrong_code() {
        let code = SessionCode("K7QP-2M9X".into());
        assert!(!code.verify("K7QP-2M9Y"));
        assert!(!code.verify("short"));
        assert!(!code.verify(""));
    }

    #[test]
    fn generated_codes_are_not_constant() {
        let a = SessionCode::generate();
        let b = SessionCode::generate();
        assert_ne!(a.as_str(), b.as_str());
    }
}
