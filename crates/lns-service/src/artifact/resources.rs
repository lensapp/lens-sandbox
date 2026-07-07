use lns_artifact::spec::{Quantity, Resources};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmSize {
    pub cpus: u8,
    pub mem_mib: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResourceOverrides {
    pub cpus: Option<u8>,
    pub mem_mib: Option<usize>,
}

pub fn resolve_size(
    bundle: Option<&Resources>,
    overrides: &ResourceOverrides,
    defaults: VmSize,
) -> VmSize {
    let bundle_cpus = bundle
        .and_then(|r| r.cpu.as_ref())
        .and_then(quantity_to_cpus);
    let bundle_mem = bundle
        .and_then(|r| r.memory.as_ref())
        .and_then(quantity_to_mib);
    VmSize {
        cpus: overrides.cpus.or(bundle_cpus).unwrap_or(defaults.cpus),
        mem_mib: overrides.mem_mib.or(bundle_mem).unwrap_or(defaults.mem_mib),
    }
}

fn quantity_to_cpus(quantity: &Quantity) -> Option<u8> {
    let whole = match quantity {
        Quantity::Int(n) => u32::try_from(*n).ok()?,
        Quantity::Text(text) => parse_cpu_text(text)?,
    };
    (1..=u32::from(u8::MAX))
        .contains(&whole)
        .then_some(whole as u8)
}

fn parse_cpu_text(text: &str) -> Option<u32> {
    let text = text.trim();
    match text.strip_suffix('m') {
        Some(millis) => millis.parse::<u32>().ok().map(|m| m.div_ceil(1000)),
        None => text.parse::<u32>().ok(),
    }
}

fn quantity_to_mib(quantity: &Quantity) -> Option<usize> {
    match quantity {
        Quantity::Int(n) => (*n > 0).then_some(*n as usize),
        Quantity::Text(text) => parse_mem_text(text),
    }
}

fn parse_mem_text(text: &str) -> Option<usize> {
    let text = text.trim();
    if let Some(gib) = text.strip_suffix("Gi") {
        return gib.trim().parse::<usize>().ok().map(|v| v * 1024);
    }
    if let Some(mib) = text.strip_suffix("Mi") {
        return mib.trim().parse::<usize>().ok();
    }
    text.parse::<usize>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULTS: VmSize = VmSize {
        cpus: 1,
        mem_mib: 512,
    };

    #[test]
    fn an_absent_bundle_size_falls_back_to_defaults() {
        let size = resolve_size(None, &ResourceOverrides::default(), DEFAULTS);
        assert_eq!(size, DEFAULTS);
    }

    #[test]
    fn a_text_quantity_with_units_is_understood() {
        let res = Resources {
            cpu: Some(Quantity::Text("2".into())),
            memory: Some(Quantity::Text("2Gi".into())),
        };
        let size = resolve_size(Some(&res), &ResourceOverrides::default(), DEFAULTS);
        assert_eq!(
            size,
            VmSize {
                cpus: 2,
                mem_mib: 2048
            }
        );
    }

    #[test]
    fn millicore_cpu_rounds_up_and_units_or_plain_memory_are_mib() {
        let res = Resources {
            cpu: Some(Quantity::Text("1500m".into())),
            memory: Some(Quantity::Text("768Mi".into())),
        };
        let size = resolve_size(Some(&res), &ResourceOverrides::default(), DEFAULTS);
        assert_eq!(
            size,
            VmSize {
                cpus: 2,
                mem_mib: 768
            }
        );

        let plain = Resources {
            cpu: None,
            memory: Some(Quantity::Text("640".into())),
        };
        assert_eq!(
            resolve_size(Some(&plain), &ResourceOverrides::default(), DEFAULTS).mem_mib,
            640
        );
    }

    #[test]
    fn an_unparseable_or_out_of_range_quantity_falls_back_to_defaults() {
        let res = Resources {
            cpu: Some(Quantity::Int(0)),
            memory: Some(Quantity::Text("lots".into())),
        };
        let size = resolve_size(Some(&res), &ResourceOverrides::default(), DEFAULTS);
        assert_eq!(size, DEFAULTS);

        let too_many = Resources {
            cpu: Some(Quantity::Int(9000)),
            memory: Some(Quantity::Int(-4)),
        };
        assert_eq!(
            resolve_size(Some(&too_many), &ResourceOverrides::default(), DEFAULTS),
            DEFAULTS
        );
    }
}
