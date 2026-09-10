#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use lns_session::{BrokerExitReason, ClientFrame, GuestNet, ServerFrame};

use crate::network::Applied;

/// What the guest did with the one plan it is willing to read before it is addressed.
#[derive(Debug)]
pub enum Bootstrap {
    Applied(Applied),
    Refused(BrokerExitReason),
}

/// The guest serves nothing but this until it is addressed: any frame that is not a plan, and a host that hangs up, both end the run instead of reaching a workload on an unaddressed interface.
pub fn respond(
    frame: Option<ClientFrame>,
    apply: impl FnOnce(&GuestNet) -> Result<Applied, BrokerExitReason>,
) -> Bootstrap {
    let net = match frame {
        Some(ClientFrame::ConfigureNetwork(net)) => net,
        Some(other) => {
            return refused(format!(
                "the host sent {} before the network plan",
                frame_name(&other)
            ));
        }
        None => {
            return refused(
                "the host closed the bootstrap channel before it sent a network plan".to_string(),
            );
        }
    };
    if let Err(e) = net.validate() {
        return refused(format!("the host address plan is unusable: {e}"));
    }
    match apply(&net) {
        Ok(applied) => Bootstrap::Applied(applied),
        Err(reason) => Bootstrap::Refused(reason),
    }
}

/// One console line, for the operator reading the guest's own log.
pub fn narrate(outcome: &Bootstrap) -> String {
    match outcome {
        Bootstrap::Applied(applied) => applied.report(),
        Bootstrap::Refused(reason) => {
            format!("refusing to start the workload: {}", reason.summary())
        }
    }
}

pub fn reply(outcome: &Bootstrap) -> ServerFrame {
    match outcome {
        Bootstrap::Applied(applied) => ServerFrame::NetworkApplied {
            address: applied.address.to_string(),
        },
        Bootstrap::Refused(reason) => ServerFrame::Refused(reason.clone()),
    }
}

fn refused(detail: String) -> Bootstrap {
    Bootstrap::Refused(BrokerExitReason::NetworkSetupFailed(detail))
}

fn frame_name(frame: &ClientFrame) -> &'static str {
    match frame {
        ClientFrame::ConfigureNetwork(_) => "a network plan",
        ClientFrame::OpenSession { .. } => "a session request",
        ClientFrame::StdinBytes(_) => "workload input",
        ClientFrame::StdinClose => "an input close",
        ClientFrame::Resize(_) => "a resize",
        ClientFrame::Signal(_) => "a signal",
        ClientFrame::Detach => "a detach",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_session::{BrokerExitReason, ClientFrame, GuestNet, ServerFrame};
    use std::cell::Cell;
    use std::net::Ipv4Addr;

    fn plan() -> GuestNet {
        GuestNet {
            candidates: vec![Ipv4Addr::new(192, 168, 64, 254)],
            prefix_len: 24,
            gateway: Ipv4Addr::new(192, 168, 64, 1),
            dns: vec![Ipv4Addr::new(192, 168, 64, 1)],
        }
    }

    fn applied() -> crate::network::Applied {
        crate::network::Applied {
            address: Ipv4Addr::new(192, 168, 64, 254),
            prefix_len: 24,
            gateway: Ipv4Addr::new(192, 168, 64, 1),
            dns: vec!["192.168.64.1".to_string()],
        }
    }

    #[test]
    fn the_plan_the_host_sends_is_applied_and_the_address_reported_back() {
        let seen = Cell::new(None);
        let outcome = respond(Some(ClientFrame::ConfigureNetwork(plan())), |net| {
            seen.set(Some(net.clone()));
            Ok(applied())
        });
        assert_eq!(seen.into_inner(), Some(plan()));
        assert_eq!(
            reply(&outcome),
            ServerFrame::NetworkApplied {
                address: "192.168.64.254".to_string()
            }
        );
        assert!(matches!(outcome, Bootstrap::Applied(_)), "{outcome:?}");
    }

    #[test]
    fn nothing_but_a_plan_opens_this_channel() {
        let outcome = respond(
            Some(ClientFrame::StdinBytes(b"whoami".to_vec())),
            |_| -> Result<crate::network::Applied, BrokerExitReason> {
                panic!("the guest configured its network for a frame that carried no plan")
            },
        );
        let Bootstrap::Refused(reason) = &outcome else {
            panic!("a workload frame must never reach an unaddressed guest: {outcome:?}");
        };
        assert_eq!(reason.as_str(), "network_setup_failed");
        assert!(
            reason.summary().contains("before the network plan"),
            "{reason:?}"
        );
    }

    #[test]
    fn a_host_that_hangs_up_before_it_sends_a_plan_leaves_a_refusal_not_a_wait() {
        let outcome = respond(
            None,
            |_| -> Result<crate::network::Applied, BrokerExitReason> {
                panic!("no plan arrived, so nothing may be configured")
            },
        );
        let Bootstrap::Refused(reason) = &outcome else {
            panic!("{outcome:?}");
        };
        assert!(reason.summary().contains("closed"), "{reason:?}");
    }

    #[test]
    fn a_plan_the_guest_cannot_use_is_refused_before_the_interface_is_touched() {
        let mut broken = plan();
        broken.candidates.clear();
        let outcome = respond(
            Some(ClientFrame::ConfigureNetwork(broken)),
            |_| -> Result<crate::network::Applied, BrokerExitReason> {
                panic!("an unusable plan must never be applied")
            },
        );
        let Bootstrap::Refused(reason) = &outcome else {
            panic!("{outcome:?}");
        };
        assert!(
            reason.summary().contains("no candidate address"),
            "{reason:?}"
        );
    }

    #[test]
    fn each_outcome_gets_one_console_line() {
        let applied = respond(Some(ClientFrame::ConfigureNetwork(plan())), |_| {
            Ok(applied())
        });
        assert!(
            narrate(&applied).contains("192.168.64.254/24"),
            "{applied:?}"
        );
        let refused = respond(
            None,
            |_| -> Result<crate::network::Applied, BrokerExitReason> {
                unreachable!("no plan arrived")
            },
        );
        assert!(narrate(&refused).contains("refusing"), "{refused:?}");
    }

    #[test]
    fn a_guest_that_cannot_take_any_offered_address_refuses_by_name() {
        let refusal = BrokerExitReason::NoStaticAddress {
            offered: vec!["192.168.64.254".to_string()],
        };
        let outcome = respond(Some(ClientFrame::ConfigureNetwork(plan())), |_| {
            Err(refusal.clone())
        });
        assert_eq!(reply(&outcome), ServerFrame::Refused(refusal));
    }
}
