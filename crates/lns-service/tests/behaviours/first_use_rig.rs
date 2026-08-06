use lns_policy::Policy;
use lns_policy::connectors::Connector;
use lns_policy::grants::WorkloadGrantFile;

/// Drives the first-use offer path: catalog + directory policy + this workload's grants in, the policy the launch actually runs and whether the request was held with an offer out.
#[derive(Default)]
pub struct FirstUseRig {
    pub catalog: Vec<Connector>,
    pub overlay: Policy,
    pub grants: WorkloadGrantFile,
    /// The policy the launch composed, after withholding the allows of any undecided connector.
    pub running_policy: Option<Policy>,
    /// Connector ids the approval card offered for the request.
    pub offered: Vec<String>,
    /// The sign-in methods the presented offer card listed, in the order the user reads them.
    pub offered_methods: Vec<String>,
    pub held: bool,
    pub proceeded: bool,
    /// The session under test, kept so a later step can answer the card it raised.
    pub session: Option<std::sync::Arc<lns_service::approval_flow::session::ApprovalSession>>,
    /// The card id the request raised.
    pub card: Option<String>,
    /// Connector ids the session declined through its connect port.
    pub declined: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl std::fmt::Debug for FirstUseRig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FirstUseRig")
            .field("offered", &self.offered)
            .field("held", &self.held)
            .field("proceeded", &self.proceeded)
            .finish_non_exhaustive()
    }
}
