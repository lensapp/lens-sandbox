use std::net::Ipv4Addr;
use std::time::Duration;

use smoltcp::wire::{DhcpMessageType, DhcpPacket, DhcpRepr, EthernetAddress};

use super::link::Mac;

pub const SERVER_PORT: u16 = 67;
pub const CLIENT_PORT: u16 = 68;

/// The one address this link hands out. There is no pool: one guest, one lease, for as long as the run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lease {
    pub gateway: Ipv4Addr,
    pub guest: Ipv4Addr,
    pub netmask: Ipv4Addr,
    pub duration: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub xid: u32,
    pub broadcast: bool,
    pub ciaddr: Ipv4Addr,
    pub chaddr: Mac,
    pub kind: DhcpMessageType,
    pub requested_ip: Option<Ipv4Addr>,
    pub server_id: Option<Ipv4Addr>,
}

/// Where a reply goes: a client with no address yet can only be reached by broadcast.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplyTo {
    Broadcast,
    Unicast { mac: Mac, ip: Ipv4Addr },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    pub payload: Vec<u8>,
    pub to: ReplyTo,
}

pub fn parse(payload: &[u8]) -> Option<Request> {
    let packet = DhcpPacket::new_checked(payload).ok()?;
    let repr = DhcpRepr::parse(&packet).ok()?;
    Some(Request {
        xid: repr.transaction_id,
        broadcast: repr.broadcast,
        ciaddr: repr.client_ip,
        chaddr: repr.client_hardware_address.0,
        kind: repr.message_type,
        requested_ip: repr.requested_ip,
        server_id: repr.server_identifier,
    })
}

/// The reply this link owes a DHCP message, or `None` when the message names another server or is not one we answer.
pub fn answer(lease: &Lease, payload: &[u8]) -> Option<Reply> {
    let request = parse(payload)?;
    if request
        .server_id
        .is_some_and(|named| named != lease.gateway)
    {
        return None;
    }
    let kind = match request.kind {
        DhcpMessageType::Discover => DhcpMessageType::Offer,
        DhcpMessageType::Request if wants_another_address(&request, lease) => DhcpMessageType::Nak,
        DhcpMessageType::Request => DhcpMessageType::Ack,
        _ => return None,
    };
    Some(Reply {
        payload: build(lease, &request, kind),
        to: reply_to(&request, lease, kind),
    })
}

fn wants_another_address(request: &Request, lease: &Lease) -> bool {
    let asked = request.requested_ip.unwrap_or(request.ciaddr);
    !asked.is_unspecified() && asked != lease.guest
}

fn reply_to(request: &Request, lease: &Lease, kind: DhcpMessageType) -> ReplyTo {
    if kind == DhcpMessageType::Nak || request.broadcast {
        return ReplyTo::Broadcast;
    }
    ReplyTo::Unicast {
        mac: request.chaddr,
        ip: if request.ciaddr.is_unspecified() {
            lease.guest
        } else {
            request.ciaddr
        },
    }
}

fn build(lease: &Lease, request: &Request, kind: DhcpMessageType) -> Vec<u8> {
    let leases = kind != DhcpMessageType::Nak;
    let granted = |address| {
        if leases {
            address
        } else {
            Ipv4Addr::UNSPECIFIED
        }
    };
    let repr = DhcpRepr {
        message_type: kind,
        transaction_id: request.xid,
        secs: 0,
        client_hardware_address: EthernetAddress(request.chaddr),
        client_ip: request.ciaddr,
        your_ip: granted(lease.guest),
        server_ip: granted(lease.gateway),
        relay_agent_ip: Ipv4Addr::UNSPECIFIED,
        broadcast: request.broadcast,
        server_identifier: Some(lease.gateway),
        router: leases.then_some(lease.gateway),
        subnet_mask: leases.then_some(lease.netmask),
        dns_servers: leases.then(|| std::iter::once(lease.gateway).collect()),
        lease_duration: leases.then(|| u32::try_from(lease.duration.as_secs()).unwrap_or(u32::MAX)),
        requested_ip: None,
        client_identifier: None,
        parameter_request_list: None,
        max_size: None,
        renew_duration: None,
        rebind_duration: None,
        additional_options: &[],
    };
    let mut out = vec![0u8; repr.buffer_len()];
    // `buffer_len` measures exactly these options, so the emit cannot run out of room.
    let _ = repr.emit(&mut DhcpPacket::new_unchecked(&mut out));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLIENT: Mac = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];

    const OP_REQUEST: u8 = 1;
    const OP_REPLY: u8 = 2;
    const HTYPE_ETHERNET: u8 = 1;
    const HLEN_ETHERNET: u8 = 6;
    const MAGIC: [u8; 4] = [99, 130, 83, 99];
    const FIXED_LEN: usize = 236;
    const BROADCAST_FLAG: u16 = 0x8000;

    const DISCOVER: u8 = 1;
    const OFFER: u8 = 2;
    const REQUEST: u8 = 3;
    const ACK: u8 = 5;
    const NAK: u8 = 6;

    const OPT_SUBNET_MASK: u8 = 1;
    const OPT_ROUTER: u8 = 3;
    const OPT_DNS: u8 = 6;
    const OPT_REQUESTED_IP: u8 = 50;
    const OPT_LEASE_TIME: u8 = 51;
    const OPT_MESSAGE_TYPE: u8 = 53;
    const OPT_SERVER_ID: u8 = 54;
    const OPT_END: u8 = 255;
    const OPT_PAD: u8 = 0;

    fn lease() -> Lease {
        Lease {
            gateway: Ipv4Addr::new(192, 168, 127, 1),
            guest: Ipv4Addr::new(192, 168, 127, 2),
            netmask: Ipv4Addr::new(255, 255, 255, 0),
            duration: Duration::from_secs(24 * 60 * 60),
        }
    }

    struct Message {
        kind: u8,
        flags: u16,
        ciaddr: Ipv4Addr,
        requested: Option<Ipv4Addr>,
        server_id: Option<Ipv4Addr>,
    }

    impl Message {
        fn of(kind: u8) -> Self {
            Self {
                kind,
                flags: BROADCAST_FLAG,
                ciaddr: Ipv4Addr::UNSPECIFIED,
                requested: None,
                server_id: None,
            }
        }

        fn bytes(&self) -> Vec<u8> {
            let mut out = vec![0u8; FIXED_LEN];
            out[0] = OP_REQUEST;
            out[1] = HTYPE_ETHERNET;
            out[2] = HLEN_ETHERNET;
            out[4..8].copy_from_slice(&0xdead_beefu32.to_be_bytes());
            out[10..12].copy_from_slice(&self.flags.to_be_bytes());
            out[12..16].copy_from_slice(&self.ciaddr.octets());
            out[28..34].copy_from_slice(&CLIENT);
            out.extend_from_slice(&MAGIC);
            push_option(&mut out, OPT_MESSAGE_TYPE, &[self.kind]);
            if let Some(ip) = self.requested {
                push_option(&mut out, OPT_REQUESTED_IP, &ip.octets());
            }
            if let Some(ip) = self.server_id {
                push_option(&mut out, OPT_SERVER_ID, &ip.octets());
            }
            out.push(OPT_END);
            out
        }
    }

    fn push_option(out: &mut Vec<u8>, code: u8, value: &[u8]) {
        out.push(code);
        out.push(value.len() as u8);
        out.extend_from_slice(value);
    }

    fn option(payload: &[u8], code: u8) -> Option<Vec<u8>> {
        let packet = DhcpPacket::new_checked(payload).expect("a reply is a dhcp packet");
        let mut options = packet.options();
        let found = options.find(|option| option.kind == code)?;
        Some(found.data.to_vec())
    }

    fn yiaddr(payload: &[u8]) -> Ipv4Addr {
        DhcpPacket::new_checked(payload)
            .expect("a reply is a dhcp packet")
            .your_ip()
    }

    #[test]
    fn a_discover_is_offered_the_one_address_this_link_leases() {
        let reply =
            answer(&lease(), &Message::of(DISCOVER).bytes()).expect("a discover is offered");

        assert_eq!(option(&reply.payload, OPT_MESSAGE_TYPE), Some(vec![OFFER]));
        assert_eq!(yiaddr(&reply.payload), Ipv4Addr::new(192, 168, 127, 2));
        assert_eq!(reply.payload[0], OP_REPLY);
        assert_eq!(
            u32::from_be_bytes(reply.payload[4..8].try_into().unwrap()),
            0xdead_beef,
            "a client matches the reply to its request by transaction id"
        );
        assert_eq!(reply.payload[28..34], CLIENT);
    }

    #[test]
    fn an_offer_carries_the_mask_router_resolver_lease_and_server_the_guest_reads() {
        let reply = answer(&lease(), &Message::of(DISCOVER).bytes()).unwrap();

        assert_eq!(
            option(&reply.payload, OPT_SUBNET_MASK),
            Some(vec![255, 255, 255, 0])
        );
        assert_eq!(
            option(&reply.payload, OPT_ROUTER),
            Some(vec![192, 168, 127, 1])
        );
        assert_eq!(
            option(&reply.payload, OPT_DNS),
            Some(vec![192, 168, 127, 1])
        );
        assert_eq!(
            option(&reply.payload, OPT_SERVER_ID),
            Some(vec![192, 168, 127, 1])
        );
        assert_eq!(
            option(&reply.payload, OPT_LEASE_TIME),
            Some(86_400u32.to_be_bytes().to_vec())
        );
    }

    #[test]
    fn an_offer_reads_back_through_the_library_that_wrote_it() {
        let reply = answer(&lease(), &Message::of(DISCOVER).bytes()).unwrap();

        let packet = DhcpPacket::new_checked(&reply.payload[..]).expect("an offer is a packet");
        let offer = DhcpRepr::parse(&packet).expect("an offer is a dhcpv4 message");

        assert_eq!(offer.message_type, DhcpMessageType::Offer);
        assert_eq!(offer.transaction_id, 0xdead_beef);
        assert_eq!(offer.secs, 0);
        assert_eq!(offer.client_hardware_address, EthernetAddress(CLIENT));
        assert_eq!(offer.client_ip, Ipv4Addr::UNSPECIFIED);
        assert_eq!(offer.your_ip, Ipv4Addr::new(192, 168, 127, 2));
        assert_eq!(offer.server_ip, Ipv4Addr::new(192, 168, 127, 1));
        assert_eq!(offer.relay_agent_ip, Ipv4Addr::UNSPECIFIED);
        assert!(offer.broadcast);
        assert_eq!(
            offer.server_identifier,
            Some(Ipv4Addr::new(192, 168, 127, 1))
        );
        assert_eq!(offer.router, Some(Ipv4Addr::new(192, 168, 127, 1)));
        assert_eq!(offer.subnet_mask, Some(Ipv4Addr::new(255, 255, 255, 0)));
        assert_eq!(
            offer.dns_servers.map(|servers| servers.to_vec()),
            Some(vec![Ipv4Addr::new(192, 168, 127, 1)])
        );
        assert_eq!(offer.lease_duration, Some(86_400));
    }

    #[test]
    fn a_request_is_acknowledged_with_the_same_address_and_options() {
        let asking = Message {
            requested: Some(Ipv4Addr::new(192, 168, 127, 2)),
            server_id: Some(Ipv4Addr::new(192, 168, 127, 1)),
            ..Message::of(REQUEST)
        };

        let reply = answer(&lease(), &asking.bytes()).expect("a request is acknowledged");

        assert_eq!(option(&reply.payload, OPT_MESSAGE_TYPE), Some(vec![ACK]));
        assert_eq!(yiaddr(&reply.payload), Ipv4Addr::new(192, 168, 127, 2));
        assert_eq!(
            option(&reply.payload, OPT_ROUTER),
            Some(vec![192, 168, 127, 1])
        );
    }

    #[test]
    fn a_renew_with_no_discover_before_it_is_acknowledged_too() {
        let renewing = Message {
            flags: 0,
            ciaddr: Ipv4Addr::new(192, 168, 127, 2),
            ..Message::of(REQUEST)
        };

        let reply = answer(&lease(), &renewing.bytes()).expect("a lease renews without a discover");

        assert_eq!(option(&reply.payload, OPT_MESSAGE_TYPE), Some(vec![ACK]));
        assert_eq!(yiaddr(&reply.payload), Ipv4Addr::new(192, 168, 127, 2));
        assert_eq!(
            reply.to,
            ReplyTo::Unicast {
                mac: CLIENT,
                ip: Ipv4Addr::new(192, 168, 127, 2)
            },
            "a client that already holds its address is answered on it"
        );
    }

    #[test]
    fn a_client_that_asks_for_another_address_is_told_no_rather_than_left_waiting() {
        let elsewhere = Message {
            requested: Some(Ipv4Addr::new(10, 0, 0, 5)),
            ..Message::of(REQUEST)
        };

        let reply = answer(&lease(), &elsewhere.bytes()).expect("a stale lease gets an answer");

        assert_eq!(option(&reply.payload, OPT_MESSAGE_TYPE), Some(vec![NAK]));
        assert_eq!(yiaddr(&reply.payload), Ipv4Addr::UNSPECIFIED);
        assert_eq!(
            option(&reply.payload, OPT_ROUTER),
            None,
            "a refusal leases nothing"
        );
        assert_eq!(reply.to, ReplyTo::Broadcast);
    }

    #[test]
    fn the_broadcast_flag_decides_where_the_reply_goes() {
        let broadcast = answer(&lease(), &Message::of(DISCOVER).bytes()).unwrap();
        assert_eq!(broadcast.to, ReplyTo::Broadcast);

        let unicast = answer(
            &lease(),
            &Message {
                flags: 0,
                ..Message::of(DISCOVER)
            }
            .bytes(),
        )
        .unwrap();
        assert_eq!(
            unicast.to,
            ReplyTo::Unicast {
                mac: CLIENT,
                ip: Ipv4Addr::new(192, 168, 127, 2)
            }
        );
    }

    #[test]
    fn the_reply_echoes_the_flags_so_a_relay_reads_them_back() {
        let reply = answer(&lease(), &Message::of(DISCOVER).bytes()).unwrap();
        assert_eq!(
            u16::from_be_bytes(reply.payload[10..12].try_into().unwrap()),
            BROADCAST_FLAG
        );
    }

    #[test]
    fn a_request_naming_another_server_is_left_to_that_server() {
        let elsewhere = Message {
            server_id: Some(Ipv4Addr::new(10, 0, 0, 1)),
            requested: Some(Ipv4Addr::new(192, 168, 127, 2)),
            ..Message::of(REQUEST)
        };
        assert_eq!(answer(&lease(), &elsewhere.bytes()), None);
    }

    #[test]
    fn a_message_this_server_does_not_answer_is_ignored() {
        for kind in [4u8, 7, 8] {
            assert_eq!(answer(&lease(), &Message::of(kind).bytes()), None);
        }
    }

    #[test]
    fn a_type_that_does_not_match_the_packet_opcode_is_not_read_as_a_request() {
        let posing_as_a_server = Message::of(OFFER).bytes();

        assert_eq!(parse(&posing_as_a_server), None);
        assert_eq!(answer(&lease(), &posing_as_a_server), None);
    }

    #[test]
    fn bytes_that_are_not_a_dhcp_request_are_ignored() {
        assert_eq!(parse(&[]), None);
        assert_eq!(
            parse(&vec![0u8; FIXED_LEN + 4]),
            None,
            "a zero op is not a request"
        );

        let mut wrong_magic = Message::of(DISCOVER).bytes();
        wrong_magic[FIXED_LEN] = 0;
        assert_eq!(parse(&wrong_magic), None);

        let mut reply_not_request = Message::of(DISCOVER).bytes();
        reply_not_request[0] = OP_REPLY;
        assert_eq!(parse(&reply_not_request), None);

        let mut not_ethernet = Message::of(DISCOVER).bytes();
        not_ethernet[1] = 6;
        assert_eq!(parse(&not_ethernet), None);

        let mut wrong_hlen = Message::of(DISCOVER).bytes();
        wrong_hlen[2] = 16;
        assert_eq!(parse(&wrong_hlen), None);
    }

    #[test]
    fn a_message_with_no_type_option_decides_nothing() {
        let mut untyped = vec![0u8; FIXED_LEN];
        untyped[0] = OP_REQUEST;
        untyped[1] = HTYPE_ETHERNET;
        untyped[2] = HLEN_ETHERNET;
        untyped.extend_from_slice(&MAGIC);
        untyped.push(OPT_END);
        assert_eq!(parse(&untyped), None);
    }

    #[test]
    fn padding_is_skipped_and_a_truncated_option_ends_the_list() {
        let mut padded = Message::of(DISCOVER).bytes();
        padded.pop();
        padded.insert(FIXED_LEN + MAGIC.len(), OPT_PAD);
        padded.push(OPT_REQUESTED_IP);
        padded.push(4);
        padded.push(192);

        let request = parse(&padded).expect("the padding does not hide the type");
        assert_eq!(request.kind, DhcpMessageType::Discover);
        assert_eq!(request.requested_ip, None, "half an option is no option");
    }

    #[test]
    fn an_option_with_no_length_byte_ends_the_list() {
        let mut truncated = Message::of(DISCOVER).bytes();
        truncated.pop();
        truncated.push(OPT_REQUESTED_IP);
        assert_eq!(
            parse(&truncated).map(|request| request.kind),
            Some(DhcpMessageType::Discover)
        );
    }

    #[test]
    fn a_lease_longer_than_the_wire_can_say_is_capped_rather_than_wrapped() {
        let forever = Lease {
            duration: Duration::from_secs(u64::from(u32::MAX) + 10),
            ..lease()
        };
        let reply = answer(&forever, &Message::of(DISCOVER).bytes()).unwrap();
        assert_eq!(
            option(&reply.payload, OPT_LEASE_TIME),
            Some(u32::MAX.to_be_bytes().to_vec())
        );
    }
}
