use std::net::Ipv4Addr;

pub fn servers(contents: &str) -> Result<Vec<Ipv4Addr>, String> {
    let mut servers = Vec::new();
    for line in contents.lines() {
        let mut fields = line
            .split('#')
            .next()
            .unwrap_or_default()
            .split_whitespace();
        if fields.next() != Some("nameserver") {
            continue;
        }
        if let Some(address) = fields
            .next()
            .and_then(|value| value.parse::<Ipv4Addr>().ok())
            && !servers.contains(&address)
        {
            servers.push(address);
        }
    }
    if servers.is_empty() || servers.len() > 8 {
        return Err("DNS needs one to eight literal IPv4 resolvers".into());
    }
    Ok(servers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolver_addresses_are_literal_deduplicated_and_required() {
        assert_eq!(servers("# nameserver 8.8.8.8\nnameserver 1.1.1.1 # pinned\nnameserver 1.1.1.1\nnameserver 8.8.4.4\nsearch example\n").unwrap(), vec![Ipv4Addr::new(1,1,1,1), Ipv4Addr::new(8,8,4,4)]);
        for contents in ["", "nameserver resolver.example", "nameserver ::1"] {
            assert!(servers(contents).is_err());
        }
    }
}
