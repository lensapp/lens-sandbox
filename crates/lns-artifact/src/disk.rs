use crate::memory::{self, ParseError};
use crate::spec::Quantity;

pub const MIN_MIB: u64 = 16;

/// The guest filesystem numbers its 4 KiB blocks in 32 bits, so 16 TiB is the first size it cannot address.
pub const MAX_MIB_EXCLUSIVE: u64 = 16 * 1024 * 1024;

/// Reads a `resources.disk` or `volumes[].size` into bytes; a bare integer is a MiB count, and a share is refused.
pub fn parse_bytes(quantity: &Quantity) -> Result<u64, ParseError> {
    let mib = match quantity {
        Quantity::Int(n) => u64::try_from(*n).unwrap_or(0),
        Quantity::Text(text) if crate::resources::parse_percent(text).is_some() => {
            return Err(ParseError::new(format!(
                "disk size `{text}` is a share; a disk takes an absolute size, because the host has already committed the space a share would name"
            )));
        }
        Quantity::Text(text) => memory::parse_mib(text)? as u64,
    };
    if mib < MIN_MIB {
        return Err(ParseError::new(format!(
            "disk size {quantity:?} must be at least 16Mi, which is what the guest filesystem needs for its own metadata"
        )));
    }
    if mib >= MAX_MIB_EXCLUSIVE {
        return Err(ParseError::new(format!(
            "disk size {quantity:?} must be less than 16Ti, which is the largest the guest filesystem can address"
        )));
    }
    Ok(mib * 1024 * 1024)
}

#[cfg(test)]
mod tests {
    use crate::disk::parse_bytes;
    use crate::spec::Quantity;

    #[test]
    fn a_byte_size_string_resolves_to_bytes() {
        assert_eq!(
            parse_bytes(&Quantity::Text("40Gi".into())).unwrap(),
            40 << 30
        );
        assert_eq!(
            parse_bytes(&Quantity::Text("16Mi".into())).unwrap(),
            16 << 20
        );
        assert_eq!(parse_bytes(&Quantity::Text("2t".into())).unwrap(), 2 << 40);
    }

    #[test]
    fn a_bare_integer_is_a_mib_count() {
        assert_eq!(parse_bytes(&Quantity::Int(100)).unwrap(), 100 << 20);
    }

    #[test]
    fn a_share_is_refused_because_a_host_cannot_honour_it() {
        let err = parse_bytes(&Quantity::Text("50%".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("share"), "{err}");
    }

    #[test]
    fn a_disk_too_small_to_hold_its_own_metadata_is_refused() {
        for quantity in [
            Quantity::Text("15Mi".into()),
            Quantity::Int(15),
            Quantity::Int(0),
        ] {
            let err = parse_bytes(&quantity).unwrap_err().to_string();
            assert!(err.contains("at least 16Mi"), "{quantity:?}: {err}");
        }
    }

    #[test]
    fn a_disk_the_guest_filesystem_cannot_address_is_refused() {
        for quantity in [Quantity::Text("16Ti".into()), Quantity::Text("17Ti".into())] {
            let err = parse_bytes(&quantity).unwrap_err().to_string();
            assert!(err.contains("less than 16Ti"), "{quantity:?}: {err}");
        }
        assert!(parse_bytes(&Quantity::Text("16777215".into())).is_ok());
    }

    #[test]
    fn a_negative_integer_is_refused_before_it_becomes_a_size() {
        let err = parse_bytes(&Quantity::Int(-1)).unwrap_err().to_string();
        assert!(err.contains("at least 16Mi"), "{err}");
    }

    #[test]
    fn an_unparsable_size_names_the_problem_back_to_the_author() {
        let err = parse_bytes(&Quantity::Text("40parsecs".into()))
            .unwrap_err()
            .to_string();
        assert!(err.contains("unknown unit"), "{err}");
    }
}
