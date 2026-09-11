use std::net::Ipv4Addr;

use etherparse::{EtherType, Ethernet2Header};

pub type Mac = [u8; 6];

pub const BROADCAST_MAC: Mac = [0xff; 6];

/// Locally administered (bit 1 of the first octet), unicast, with `6c 6e 73` spelling "lns" and `01` naming the gateway.
pub const GATEWAY_MAC: Mac = [0x0e, 0x6c, 0x6e, 0x73, 0x00, 0x01];

const ARP_LEN: usize = 28;
const ARP_ETHERNET: u16 = 1;
const ARP_REQUEST: u16 = 1;
const ARP_REPLY: u16 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArpRequest {
    pub sender_mac: Mac,
    pub sender_ip: Ipv4Addr,
    pub target_ip: Ipv4Addr,
}

pub fn parse_arp_request(payload: &[u8]) -> Option<ArpRequest> {
    let arp: &[u8; ARP_LEN] = payload.get(..ARP_LEN)?.try_into().ok()?;
    let hardware = u16::from_be_bytes([arp[0], arp[1]]);
    let protocol = u16::from_be_bytes([arp[2], arp[3]]);
    let operation = u16::from_be_bytes([arp[6], arp[7]]);
    if hardware != ARP_ETHERNET
        || protocol != EtherType::IPV4.0
        || arp[4] != 6
        || arp[5] != 4
        || operation != ARP_REQUEST
    {
        return None;
    }
    Some(ArpRequest {
        sender_mac: arp[8..14].try_into().ok()?,
        sender_ip: ipv4(&arp[14..18])?,
        target_ip: ipv4(&arp[24..28])?,
    })
}

fn ipv4(bytes: &[u8]) -> Option<Ipv4Addr> {
    let octets: [u8; 4] = bytes.try_into().ok()?;
    Some(Ipv4Addr::from(octets))
}

fn arp_reply_payload(gateway_mac: Mac, gateway_ip: Ipv4Addr, request: &ArpRequest) -> Vec<u8> {
    let mut out = Vec::with_capacity(ARP_LEN);
    out.extend_from_slice(&ARP_ETHERNET.to_be_bytes());
    out.extend_from_slice(&EtherType::IPV4.0.to_be_bytes());
    out.push(6);
    out.push(4);
    out.extend_from_slice(&ARP_REPLY.to_be_bytes());
    out.extend_from_slice(&gateway_mac);
    out.extend_from_slice(&gateway_ip.octets());
    out.extend_from_slice(&request.sender_mac);
    out.extend_from_slice(&request.sender_ip.octets());
    out
}

pub fn wrap(source: Mac, destination: Mac, ether_type: EtherType, payload: &[u8]) -> Vec<u8> {
    let header = Ethernet2Header {
        source,
        destination,
        ether_type,
    };
    let mut frame = Vec::with_capacity(Ethernet2Header::LEN + payload.len());
    frame.extend_from_slice(&header.to_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// What a frame off the guest's link turns into: an answer to send back, an IPv4 packet to route, or nothing.
#[derive(Debug, PartialEq, Eq)]
pub enum Received<'a> {
    Answer(Vec<u8>),
    Ipv4(&'a [u8]),
    Dropped,
}

/// One guest's ethernet link: it learns the guest's MAC from the frames it sends and answers ARP for the gateway itself.
pub struct Link {
    gateway_mac: Mac,
    gateway_ip: Ipv4Addr,
    guest_mac: Option<Mac>,
}

impl Link {
    pub fn new(gateway_mac: Mac, gateway_ip: Ipv4Addr) -> Self {
        Self {
            gateway_mac,
            gateway_ip,
            guest_mac: None,
        }
    }

    pub fn guest_mac(&self) -> Option<Mac> {
        self.guest_mac
    }

    pub fn receive<'a>(&mut self, frame: &'a [u8]) -> Received<'a> {
        let Ok((header, payload)) = Ethernet2Header::from_slice(frame) else {
            return Received::Dropped;
        };
        self.guest_mac.get_or_insert(header.source);
        match header.ether_type {
            EtherType::ARP => self.answer_arp(payload),
            EtherType::IPV4 => Received::Ipv4(payload),
            _ => Received::Dropped,
        }
    }

    fn answer_arp(&self, payload: &[u8]) -> Received<'static> {
        let Some(request) = parse_arp_request(payload) else {
            return Received::Dropped;
        };
        if request.target_ip != self.gateway_ip {
            return Received::Dropped;
        }
        Received::Answer(wrap(
            self.gateway_mac,
            request.sender_mac,
            EtherType::ARP,
            &arp_reply_payload(self.gateway_mac, self.gateway_ip, &request),
        ))
    }

    /// An IPv4 packet for the guest, addressed from the gateway. Until the guest has spoken and named its own MAC, it is broadcast.
    pub fn send_ipv4(&self, packet: &[u8]) -> Vec<u8> {
        wrap(
            self.gateway_mac,
            self.guest_mac.unwrap_or(BROADCAST_MAC),
            EtherType::IPV4,
            packet,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUEST_MAC: Mac = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
    const GATEWAY: Ipv4Addr = Ipv4Addr::new(192, 168, 127, 1);

    fn answered(received: Received<'_>) -> Option<Vec<u8>> {
        match received {
            Received::Answer(frame) => Some(frame),
            _ => None,
        }
    }

    fn arp_request(target: Ipv4Addr) -> Vec<u8> {
        let request = ArpRequest {
            sender_mac: GUEST_MAC,
            sender_ip: Ipv4Addr::new(192, 168, 127, 2),
            target_ip: target,
        };
        let mut payload = arp_reply_payload(GUEST_MAC, request.sender_ip, &request);
        payload[6..8].copy_from_slice(&ARP_REQUEST.to_be_bytes());
        payload[18..24].copy_from_slice(&[0u8; 6]);
        payload[24..28].copy_from_slice(&target.octets());
        wrap(GUEST_MAC, BROADCAST_MAC, EtherType::ARP, &payload)
    }

    #[test]
    fn the_guest_mac_is_learned_from_the_first_frame_it_sends() {
        let mut link = Link::new(GATEWAY_MAC, GATEWAY);
        assert_eq!(
            link.guest_mac(),
            None,
            "nothing is known before the guest speaks"
        );

        link.receive(&wrap(GUEST_MAC, BROADCAST_MAC, EtherType::IPV4, &[0x45]));

        assert_eq!(link.guest_mac(), Some(GUEST_MAC));
    }

    #[test]
    fn a_later_frame_does_not_move_the_link_to_another_mac() {
        let mut link = Link::new(GATEWAY_MAC, GATEWAY);
        link.receive(&wrap(GUEST_MAC, BROADCAST_MAC, EtherType::IPV4, &[0x45]));
        link.receive(&wrap([0x02; 6], BROADCAST_MAC, EtherType::IPV4, &[0x45]));

        assert_eq!(
            link.guest_mac(),
            Some(GUEST_MAC),
            "one link carries one guest; a second source address is not a second guest"
        );
    }

    #[test]
    fn an_arp_request_for_the_gateway_is_answered_with_the_gateway_mac() {
        let mut link = Link::new(GATEWAY_MAC, GATEWAY);

        let frame = answered(link.receive(&arp_request(GATEWAY)))
            .expect("the guest cannot route without an answer for its gateway");

        let (header, payload) = Ethernet2Header::from_slice(&frame).unwrap();
        assert_eq!(header.source, GATEWAY_MAC);
        assert_eq!(header.destination, GUEST_MAC, "the reply is unicast back");
        assert_eq!(header.ether_type, EtherType::ARP);
        assert_eq!(u16::from_be_bytes([payload[6], payload[7]]), ARP_REPLY);
        assert_eq!(payload[8..14], GATEWAY_MAC);
        assert_eq!(payload[14..18], GATEWAY.octets());
        assert_eq!(payload[18..24], GUEST_MAC);
    }

    #[test]
    fn an_arp_request_for_any_other_address_is_dropped() {
        let mut link = Link::new(GATEWAY_MAC, GATEWAY);
        assert_eq!(
            answered(link.receive(&arp_request(Ipv4Addr::new(192, 168, 127, 254)))),
            None,
            "there is no host on this link but the gateway"
        );
    }

    #[test]
    fn an_arp_reply_is_not_answered_with_another_reply() {
        let mut link = Link::new(GATEWAY_MAC, GATEWAY);
        let mut frame = arp_request(GATEWAY);
        frame[Ethernet2Header::LEN + 6..Ethernet2Header::LEN + 8]
            .copy_from_slice(&ARP_REPLY.to_be_bytes());
        assert_eq!(link.receive(&frame), Received::Dropped);
    }

    #[test]
    fn an_arp_payload_too_short_to_read_is_dropped() {
        let mut link = Link::new(GATEWAY_MAC, GATEWAY);
        assert_eq!(
            link.receive(&wrap(GUEST_MAC, BROADCAST_MAC, EtherType::ARP, &[0; 10])),
            Received::Dropped
        );
    }

    #[test]
    fn arp_over_a_hardware_or_protocol_this_link_does_not_speak_is_dropped() {
        for (offset, value) in [(0u16, 6u16), (2, 0x86dd)] {
            let mut frame = arp_request(GATEWAY);
            let at = Ethernet2Header::LEN + offset as usize;
            frame[at..at + 2].copy_from_slice(&value.to_be_bytes());
            let mut link = Link::new(GATEWAY_MAC, GATEWAY);
            assert_eq!(link.receive(&frame), Received::Dropped);
        }
    }

    #[test]
    fn arp_with_address_lengths_that_are_not_ethernet_over_ipv4_is_dropped() {
        for (offset, value) in [(4usize, 8u8), (5, 16)] {
            let mut frame = arp_request(GATEWAY);
            frame[Ethernet2Header::LEN + offset] = value;
            let mut link = Link::new(GATEWAY_MAC, GATEWAY);
            assert_eq!(link.receive(&frame), Received::Dropped);
        }
    }

    #[test]
    fn an_ipv4_frame_hands_its_packet_on_to_be_routed() {
        let mut link = Link::new(GATEWAY_MAC, GATEWAY);
        let packet = [0x45, 0x00, 0x00, 0x14];
        assert_eq!(
            link.receive(&wrap(GUEST_MAC, GATEWAY_MAC, EtherType::IPV4, &packet)),
            Received::Ipv4(&packet)
        );
    }

    #[test]
    fn ipv6_is_dropped_because_this_link_is_ipv4_only() {
        let mut link = Link::new(GATEWAY_MAC, GATEWAY);
        assert_eq!(
            link.receive(&wrap(GUEST_MAC, GATEWAY_MAC, EtherType::IPV6, &[0x60])),
            Received::Dropped
        );
    }

    #[test]
    fn bytes_too_short_to_be_a_frame_are_dropped_without_learning_a_mac() {
        let mut link = Link::new(GATEWAY_MAC, GATEWAY);
        assert_eq!(link.receive(&[0x01, 0x02]), Received::Dropped);
        assert_eq!(link.guest_mac(), None);
    }

    #[test]
    fn an_outbound_packet_goes_to_the_guest_the_link_learned() {
        let mut link = Link::new(GATEWAY_MAC, GATEWAY);
        link.receive(&wrap(GUEST_MAC, BROADCAST_MAC, EtherType::IPV4, &[0x45]));

        let frame = link.send_ipv4(&[0x45]);

        let (header, payload) = Ethernet2Header::from_slice(&frame).unwrap();
        assert_eq!(header.source, GATEWAY_MAC);
        assert_eq!(header.destination, GUEST_MAC);
        assert_eq!(header.ether_type, EtherType::IPV4);
        assert_eq!(payload, &[0x45]);
    }

    #[test]
    fn a_packet_for_a_guest_that_has_not_spoken_yet_is_broadcast() {
        let frame = Link::new(GATEWAY_MAC, GATEWAY).send_ipv4(&[0x45]);
        let (header, _) = Ethernet2Header::from_slice(&frame).unwrap();
        assert_eq!(header.destination, BROADCAST_MAC);
        assert_eq!(header.source, GATEWAY_MAC);
    }

    #[test]
    fn the_gateway_mac_is_locally_administered_and_unicast() {
        assert_eq!(
            GATEWAY_MAC[0] & 0b10,
            0b10,
            "bit 1 marks it locally administered"
        );
        assert_eq!(GATEWAY_MAC[0] & 1, 0, "bit 0 clear marks it unicast");
    }
}
