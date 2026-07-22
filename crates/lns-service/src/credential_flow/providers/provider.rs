use crate::approval_flow::protocol::CredentialInjection;
use crate::credential_flow::detection::HostDetector;

pub trait Provider: Send + Sync + 'static {
    /// Never changes for a shipped provider: it is the persisted rule key, and renaming it strands existing developer rules.
    fn id(&self) -> &str;

    fn env_var(&self) -> &str;

    /// Must share a real token's prefix/length so shape validators accept it, yet contain `LNSPLACEHOLDER` so the registry safety test can prove no real credential leaked in.
    fn placeholder(&self) -> &str;

    fn detector(&self) -> &dyn HostDetector;

    /// Called only when the provider is armed; the unarmed case goes through [`Self::unarmed_injections`].
    fn injections(&self, value: &str) -> Vec<CredentialInjection>;

    /// Same per-domain shape with an empty `value`, so the guest declares the domain and gates the placeholder's first, pre-arm use instead of leaking it past an un-intercepted host.
    fn unarmed_injections(&self) -> Vec<CredentialInjection>;

    /// Whether the guest seeds this provider's placeholder into the workload env; a declared connector seeds, a merely-connectable one stays detect/offer-only so undeclared services don't pollute the workload env.
    fn seeds_env(&self) -> bool;
}
