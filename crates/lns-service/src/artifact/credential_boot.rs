use lns_artifact::spec::CredentialSlot;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub placeholder: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectPrompt {
    pub integration: String,
    pub env: String,
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotPlan {
    Armed { env: String, placeholder: String },
    Connect(ConnectPrompt),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootGate {
    StartWorkload,
    AwaitConnect,
}

pub fn plan_slot(slot: &CredentialSlot, binding: Option<Binding>) -> SlotPlan {
    match binding {
        Some(binding) => SlotPlan::Armed {
            env: slot.env.clone(),
            placeholder: binding.placeholder,
        },
        None => SlotPlan::Connect(ConnectPrompt {
            integration: slot.name.clone(),
            env: slot.env.clone(),
            required: slot.required,
        }),
    }
}

pub fn boot_gate(plans: &[SlotPlan]) -> BootGate {
    if plans.iter().all(|p| matches!(p, SlotPlan::Armed { .. })) {
        BootGate::StartWorkload
    } else {
        BootGate::AwaitConnect
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectChoice {
    Connect,
    Decline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotOutcome {
    Connected,
    LeftUnbound,
    AbortLaunch,
}

impl SlotOutcome {
    pub fn starts_workload(self) -> bool {
        !matches!(self, SlotOutcome::AbortLaunch)
    }
}

pub fn resolve_connect(prompt: &ConnectPrompt, choice: ConnectChoice) -> SlotOutcome {
    match choice {
        ConnectChoice::Connect => SlotOutcome::Connected,
        ConnectChoice::Decline if prompt.required => SlotOutcome::AbortLaunch,
        ConnectChoice::Decline => SlotOutcome::LeftUnbound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(required: bool) -> CredentialSlot {
        CredentialSlot {
            name: "some-provider".into(),
            env: "SOME_TOKEN".into(),
            required,
        }
    }

    #[test]
    fn an_unbound_slot_forms_a_connect_prompt_that_discloses_its_target() {
        let plan = plan_slot(&slot(true), None);
        assert_eq!(
            plan,
            SlotPlan::Connect(ConnectPrompt {
                integration: "some-provider".into(),
                env: "SOME_TOKEN".into(),
                required: true,
            })
        );
    }

    #[test]
    fn connecting_an_unbound_slot_binds_it_and_starts_the_workload() {
        let prompt = ConnectPrompt {
            integration: "some-provider".into(),
            env: "SOME_TOKEN".into(),
            required: true,
        };
        let outcome = resolve_connect(&prompt, ConnectChoice::Connect);
        assert_eq!(outcome, SlotOutcome::Connected);
        assert!(outcome.starts_workload());
    }
}
