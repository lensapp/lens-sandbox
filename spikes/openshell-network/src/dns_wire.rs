use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::{DNSClass, RData, Record, RecordType, rdata::A};
use std::future::Future;
use std::net::IpAddr;

pub struct Reply {
    pub bytes: Vec<u8>,
    pub denied: Option<String>,
}

pub async fn answer<F: Future<Output = Result<Vec<IpAddr>, String>>>(
    packet: &[u8],
    eligible: impl Fn(&str) -> bool,
    resolve: impl FnOnce(String) -> F,
) -> Option<Reply> {
    if packet.len() > 4096 {
        return None;
    }
    let request = Message::from_vec(packet).ok()?;
    if request.metadata.message_type != MessageType::Query
        || request.metadata.op_code != OpCode::Query
    {
        return None;
    }
    let [query] = request.queries.as_slice() else {
        return None;
    };
    if query.query_class() != DNSClass::IN {
        return None;
    }
    let host = query
        .name()
        .to_ascii()
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let mut response = Message::new(request.metadata.id, MessageType::Response, OpCode::Query);
    response.metadata.recursion_desired = request.metadata.recursion_desired;
    response.metadata.recursion_available = true;
    response.add_query(query.clone());
    let denied = if !eligible(&host) {
        response.metadata.response_code = ResponseCode::NXDomain;
        Some(host)
    } else if query.query_type() == RecordType::A {
        match resolve(host.clone()).await {
            Ok(addresses) if eligible(&host) => {
                for address in addresses.into_iter().take(16) {
                    if let IpAddr::V4(address) = address {
                        response.add_answer(Record::from_rdata(
                            query.name().clone(),
                            0,
                            RData::A(A(address)),
                        ));
                    }
                }
                None
            }
            _ => {
                response.metadata.response_code = ResponseCode::ServFail;
                Some(host)
            }
        }
    } else {
        None
    };
    Some(Reply {
        bytes: response.to_vec().ok()?,
        denied,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::Query;
    use hickory_proto::rr::Name;
    use std::cell::Cell;

    fn query(kind: RecordType) -> Vec<u8> {
        let mut query = Message::new(42, MessageType::Query, OpCode::Query);
        query.add_query(Query::query(
            Name::from_ascii("api.example.").unwrap(),
            kind,
        ));
        query.to_vec().unwrap()
    }

    #[tokio::test]
    async fn allowed_answers_and_policy_withdrawal_are_rechecked() {
        for withdraw in [false, true] {
            let allowed = Cell::new(true);
            let reply = answer(
                &query(RecordType::A),
                |_| allowed.get(),
                |_| async {
                    allowed.set(!withdraw);
                    Ok(vec!["203.0.113.7".parse().unwrap()])
                },
            )
            .await
            .unwrap();
            let response = Message::from_vec(&reply.bytes).unwrap();
            assert_eq!(response.answers.len(), usize::from(!withdraw));
            assert_eq!(response.metadata.id, 42);
            assert_eq!(reply.denied.is_some(), withdraw);
        }
    }

    #[tokio::test]
    async fn non_a_records_do_not_trigger_external_lookups() {
        let calls = Cell::new(0);
        for kind in [
            RecordType::AAAA,
            RecordType::TXT,
            RecordType::HTTPS,
            RecordType::ANY,
        ] {
            let reply = answer(
                &query(kind),
                |_| true,
                |_| async {
                    calls.set(calls.get() + 1);
                    Ok(vec![])
                },
            )
            .await
            .unwrap();
            let response = Message::from_vec(&reply.bytes).unwrap();
            assert!(response.answers.is_empty());
            assert_eq!(response.metadata.response_code, ResponseCode::NoError);
        }
        assert_eq!(calls.get(), 0);
    }

    #[tokio::test]
    async fn malformed_and_multi_question_packets_are_not_forwarded() {
        let mut multi = Message::from_vec(&query(RecordType::A)).unwrap();
        multi.add_query(multi.queries[0].clone());
        let calls = Cell::new(0);
        for packet in [vec![0], vec![0; 4097], multi.to_vec().unwrap()] {
            assert!(
                answer(
                    &packet,
                    |_| true,
                    |_| async {
                        calls.set(calls.get() + 1);
                        Ok(vec![])
                    }
                )
                .await
                .is_none()
            );
        }
        assert_eq!(calls.get(), 0);
    }

    #[tokio::test]
    async fn refused_dns_returns_nxdomain_without_resolving() {
        let calls = Cell::new(0);
        let reply = answer(
            &query(RecordType::A),
            |_| false,
            |_| async {
                calls.set(calls.get() + 1);
                Ok(vec![])
            },
        )
        .await
        .expect("denied DNS needs a failure response");
        assert_eq!(calls.get(), 0);
        assert_eq!(
            Message::from_vec(&reply.bytes)
                .unwrap()
                .metadata
                .response_code,
            ResponseCode::NXDomain
        );
        assert_eq!(reply.denied.as_deref(), Some("api.example"));
    }
}
