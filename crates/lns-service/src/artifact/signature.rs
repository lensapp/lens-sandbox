#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureStatus {
    SignedByTrusted,
    SignedByUntrusted,
    Unsigned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    Unsigned,
    UntrustedSigner,
}

impl RefusalReason {
    pub fn as_message(self) -> &'static str {
        match self {
            RefusalReason::Unsigned => "bundle is unsigned and a trusted signer key is configured",
            RefusalReason::UntrustedSigner => {
                "bundle is signed by a key that is not in the trusted signer set"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    Verified,
    ProceedUnverified { warning: String },
    Skipped,
    Refused(RefusalReason),
}

impl Verdict {
    pub fn allows_launch(&self) -> bool {
        !matches!(self, Verdict::Refused(_))
    }
}

const UNVERIFIABLE_WARNING: &str =
    "bundle signature cannot be verified: no trusted signer key is configured";

pub fn gate(insecure: bool, trusted_keys_configured: bool, status: SignatureStatus) -> Verdict {
    if insecure {
        return Verdict::Skipped;
    }
    if !trusted_keys_configured {
        return Verdict::ProceedUnverified {
            warning: UNVERIFIABLE_WARNING.to_string(),
        };
    }
    match status {
        SignatureStatus::SignedByTrusted => Verdict::Verified,
        SignatureStatus::Unsigned => Verdict::Refused(RefusalReason::Unsigned),
        SignatureStatus::SignedByUntrusted => Verdict::Refused(RefusalReason::UntrustedSigner),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insecure_skips_verification_even_with_a_trusted_key_and_no_signature() {
        assert_eq!(
            gate(true, true, SignatureStatus::Unsigned),
            Verdict::Skipped
        );
        assert!(gate(true, true, SignatureStatus::Unsigned).allows_launch());
    }

    #[test]
    fn a_refusal_names_which_shortfall_tripped_it() {
        assert!(RefusalReason::Unsigned.as_message().contains("unsigned"));
        assert!(
            RefusalReason::UntrustedSigner
                .as_message()
                .contains("not in the trusted")
        );
        assert_ne!(
            RefusalReason::Unsigned.as_message(),
            RefusalReason::UntrustedSigner.as_message(),
        );
    }
}
