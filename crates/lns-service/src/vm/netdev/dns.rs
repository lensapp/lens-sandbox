use std::net::{IpAddr, SocketAddr};

use futures_util::future::BoxFuture;
use hickory_proto::op::{Message, MessageType, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA};
use hickory_proto::rr::{RData, Record, RecordType};

pub const PORT: u16 = 53;

/// Short enough that a VPN coming up mid-run is not papered over by the guest's cache.
const ANSWER_TTL: u32 = 60;

/// How the gateway resolves. `lookup` is the host's own resolver, so a VPN or a scoped resolver answers; `forward` carries everything else.
pub trait Resolver: Send + Sync + 'static {
    fn lookup(&self, name: String) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>>;
    fn forward(&self, query: Vec<u8>) -> BoxFuture<'static, std::io::Result<Vec<u8>>>;
}

pub async fn answer(request: &[u8], resolver: &dyn Resolver) -> Option<Vec<u8>> {
    let message = Message::from_vec(request).ok()?;
    if message.metadata.message_type != MessageType::Query {
        return None;
    }
    let query = message.queries.first()?.clone();
    match query.query_type() {
        RecordType::A | RecordType::AAAA => {
            Some(resolved(&message, resolver, query.query_type()).await)
        }
        _ => Some(forwarded(request, &message, resolver).await),
    }
}

async fn resolved(request: &Message, resolver: &dyn Resolver, wanted: RecordType) -> Vec<u8> {
    let query = request.queries[0].clone();
    let name = query.name().to_string();
    let Ok(addresses) = resolver
        .lookup(name.trim_end_matches('.').to_string())
        .await
    else {
        return encode(response(request, ResponseCode::ServFail, Vec::new()));
    };
    let answers = addresses
        .into_iter()
        .filter_map(|address| match (address, wanted) {
            (IpAddr::V4(v4), RecordType::A) => Some(RData::A(A(v4))),
            (IpAddr::V6(v6), RecordType::AAAA) => Some(RData::AAAA(AAAA(v6))),
            _ => None,
        })
        .map(|data| Record::from_rdata(query.name().clone(), ANSWER_TTL, data))
        .collect();
    encode(response(request, ResponseCode::NoError, answers))
}

async fn forwarded(raw: &[u8], request: &Message, resolver: &dyn Resolver) -> Vec<u8> {
    match resolver.forward(raw.to_vec()).await {
        Ok(reply) => reply,
        Err(_) => encode(response(request, ResponseCode::ServFail, Vec::new())),
    }
}

fn response(request: &Message, code: ResponseCode, answers: Vec<Record>) -> Message {
    let mut reply = Message::new(
        request.metadata.id,
        MessageType::Response,
        request.metadata.op_code,
    );
    reply.metadata.recursion_desired = request.metadata.recursion_desired;
    reply.metadata.recursion_available = true;
    reply.metadata.response_code = code;
    reply.queries = request.queries.clone();
    reply.answers = answers;
    reply
}

fn encode(message: Message) -> Vec<u8> {
    message.to_vec().unwrap_or_default()
}

pub fn first_nameserver(resolv_conf: &str) -> Option<SocketAddr> {
    resolv_conf
        .lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter_map(|line| line.strip_prefix("nameserver"))
        .filter_map(|rest| rest.split_whitespace().next())
        .find_map(|address| address.parse::<IpAddr>().ok())
        .map(|address| SocketAddr::new(address, PORT))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_proto::op::{OpCode, Query};
    use hickory_proto::rr::Name;
    use std::sync::{Arc, Mutex};

    struct FakeResolver {
        addresses: Vec<IpAddr>,
        forwarded: Arc<Mutex<Vec<Vec<u8>>>>,
        reply: Option<Vec<u8>>,
    }

    impl FakeResolver {
        fn answering(addresses: &[&str]) -> Self {
            Self {
                addresses: addresses.iter().map(|a| a.parse().unwrap()).collect(),
                forwarded: Arc::new(Mutex::new(Vec::new())),
                reply: Some(b"upstream".to_vec()),
            }
        }

        fn failing() -> Self {
            Self {
                addresses: Vec::new(),
                forwarded: Arc::new(Mutex::new(Vec::new())),
                reply: None,
            }
        }
    }

    impl Resolver for FakeResolver {
        fn lookup(&self, _name: String) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>> {
            let addresses = self.addresses.clone();
            let known = self.reply.is_some();
            Box::pin(async move {
                if known {
                    Ok(addresses)
                } else {
                    Err(std::io::Error::other("no resolver"))
                }
            })
        }

        fn forward(&self, query: Vec<u8>) -> BoxFuture<'static, std::io::Result<Vec<u8>>> {
            self.forwarded.lock().unwrap().push(query);
            let reply = self.reply.clone();
            Box::pin(async move { reply.ok_or_else(|| std::io::Error::other("no nameserver")) })
        }
    }

    fn question(name: &str, kind: RecordType) -> Vec<u8> {
        let mut message = Message::new(0x4242, MessageType::Query, OpCode::Query);
        message.metadata.recursion_desired = true;
        message.add_query(Query::query(Name::from_ascii(name).unwrap(), kind));
        message.to_vec().unwrap()
    }

    async fn ask(name: &str, kind: RecordType, resolver: &dyn Resolver) -> Message {
        let reply = answer(&question(name, kind), resolver)
            .await
            .expect("a question gets an answer");
        Message::from_vec(&reply).expect("the answer is a DNS message")
    }

    #[tokio::test]
    async fn an_a_question_is_answered_from_the_hosts_own_resolver() {
        let reply = ask(
            "example.com.",
            RecordType::A,
            &FakeResolver::answering(&["93.184.216.34", "2606:2800::1"]),
        )
        .await;

        assert_eq!(
            reply.answers.len(),
            1,
            "an A question takes only the A answers"
        );
        assert_eq!(
            reply.answers[0].data,
            RData::A(A("93.184.216.34".parse().unwrap()))
        );
        assert_eq!(reply.answers[0].ttl, ANSWER_TTL);
        assert_eq!(reply.answers[0].name.to_string(), "example.com.");
    }

    #[tokio::test]
    async fn an_aaaa_question_takes_the_v6_addresses_of_the_same_lookup() {
        let reply = ask(
            "example.com.",
            RecordType::AAAA,
            &FakeResolver::answering(&["93.184.216.34", "2606:2800::1"]),
        )
        .await;

        assert_eq!(reply.answers.len(), 1);
        assert_eq!(
            reply.answers[0].data,
            RData::AAAA(AAAA("2606:2800::1".parse().unwrap()))
        );
    }

    #[tokio::test]
    async fn the_answer_carries_the_question_back_with_its_own_id() {
        let reply = ask(
            "example.com.",
            RecordType::A,
            &FakeResolver::answering(&["1.2.3.4"]),
        )
        .await;

        assert_eq!(
            reply.metadata.id, 0x4242,
            "a client matches on the id alone"
        );
        assert_eq!(reply.metadata.message_type, MessageType::Response);
        assert!(
            reply.metadata.recursion_available,
            "this gateway recurses for the guest"
        );
        assert!(
            reply.metadata.recursion_desired,
            "the request's own bit comes back"
        );
        assert_eq!(reply.queries.len(), 1);
        assert_eq!(reply.queries[0].name().to_string(), "example.com.");
    }

    #[tokio::test]
    async fn a_name_the_host_cannot_resolve_is_a_server_failure_not_a_silent_drop() {
        let reply = ask("nowhere.invalid.", RecordType::A, &FakeResolver::failing()).await;

        assert_eq!(reply.metadata.response_code, ResponseCode::ServFail);
        assert!(reply.answers.is_empty());
    }

    #[tokio::test]
    async fn a_name_with_no_address_of_the_asked_family_answers_with_no_records() {
        let reply = ask(
            "example.com.",
            RecordType::AAAA,
            &FakeResolver::answering(&["1.2.3.4"]),
        )
        .await;

        assert_eq!(reply.metadata.response_code, ResponseCode::NoError);
        assert!(reply.answers.is_empty(), "no AAAA is not a failure");
    }

    #[tokio::test]
    async fn any_other_question_type_goes_to_the_upstream_nameserver_verbatim() {
        let resolver = FakeResolver::answering(&[]);
        let query = question("example.com.", RecordType::MX);

        let reply = answer(&query, &resolver).await.expect("MX is answered too");

        assert_eq!(
            reply,
            b"upstream".to_vec(),
            "the upstream's own bytes come back"
        );
        assert_eq!(resolver.forwarded.lock().unwrap().as_slice(), &[query]);
    }

    #[tokio::test]
    async fn a_forward_with_no_nameserver_to_ask_is_a_server_failure() {
        let reply = ask("example.com.", RecordType::MX, &FakeResolver::failing()).await;

        assert_eq!(reply.metadata.response_code, ResponseCode::ServFail);
        assert_eq!(reply.metadata.id, 0x4242);
    }

    #[tokio::test]
    async fn bytes_that_are_not_a_question_are_dropped() {
        let resolver = FakeResolver::answering(&["1.2.3.4"]);
        assert_eq!(answer(b"not dns at all", &resolver).await, None);

        let empty = Message::new(1, MessageType::Query, OpCode::Query);
        assert_eq!(
            answer(&empty.to_vec().unwrap(), &resolver).await,
            None,
            "a message with no question asks nothing"
        );

        let mut response = Message::new(2, MessageType::Response, OpCode::Query);
        response.add_query(Query::query(
            Name::from_ascii("example.com.").unwrap(),
            RecordType::A,
        ));
        assert_eq!(
            answer(&response.to_vec().unwrap(), &resolver).await,
            None,
            "the gateway answers questions, not answers"
        );
    }

    #[test]
    fn the_first_nameserver_of_resolv_conf_is_the_one_forwards_go_to() {
        let contents = "# a comment\nsearch example.com\nnameserver 1.1.1.1\nnameserver 8.8.8.8\n";
        assert_eq!(
            first_nameserver(contents),
            Some("1.1.1.1:53".parse().unwrap())
        );
    }

    #[test]
    fn a_resolv_conf_with_no_usable_nameserver_names_none() {
        for contents in [
            "",
            "search example.com\n",
            "nameserver\n",
            "nameserver not-an-address\n",
        ] {
            assert_eq!(first_nameserver(contents), None, "{contents:?}");
        }
    }

    #[test]
    fn a_commented_out_nameserver_is_not_a_nameserver() {
        assert_eq!(
            first_nameserver("#nameserver 1.1.1.1\nnameserver 9.9.9.9\n"),
            Some("9.9.9.9:53".parse().unwrap())
        );
    }

    #[test]
    fn an_ipv6_nameserver_is_taken_as_written() {
        assert_eq!(
            first_nameserver("nameserver 2606:4700:4700::1111\n"),
            Some("[2606:4700:4700::1111]:53".parse().unwrap())
        );
    }
}
