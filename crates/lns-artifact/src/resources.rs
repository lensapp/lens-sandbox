use crate::spec::{Quantity, Resources};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VmSize {
    pub cpus: u8,
    pub mem_mib: usize,
}

pub const DEFAULT_VM_SIZE: VmSize = VmSize {
    cpus: 1,
    mem_mib: 512,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResourceOverrides {
    pub cpus: Option<u8>,
    pub mem_mib: Option<usize>,
}

/// What a definition asked for, already read and range-checked — `None` where it asked for nothing the host can grant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeclaredSize {
    pub cpus: Option<u8>,
    pub mem_mib: Option<usize>,
}

/// The size a run boots with, from the same expression wherever it is asked — a flag beats the definition, which beats the built-in default.
pub fn resolve_declared(
    declared: DeclaredSize,
    overrides: &ResourceOverrides,
    defaults: VmSize,
) -> VmSize {
    VmSize {
        cpus: overrides.cpus.or(declared.cpus).unwrap_or(defaults.cpus),
        mem_mib: overrides
            .mem_mib
            .or(declared.mem_mib)
            .unwrap_or(defaults.mem_mib),
    }
}

impl DeclaredSize {
    /// The `ignored` field names each request the host will not grant, so the caller that owns a log can say so.
    pub fn from_resources(resources: Option<&Resources>) -> (Self, Vec<&'static str>) {
        let mut ignored = Vec::new();
        let cpus = read(resources.and_then(|r| r.cpu.as_ref()), quantity_to_cpus);
        let mem_mib = read(resources.and_then(|r| r.memory.as_ref()), quantity_to_mib);
        if cpus.asked_and_refused {
            ignored.push("cpu");
        }
        if mem_mib.asked_and_refused {
            ignored.push("memory");
        }
        (
            Self {
                cpus: cpus.value,
                mem_mib: mem_mib.value,
            },
            ignored,
        )
    }
}

struct Read<T> {
    value: Option<T>,
    asked_and_refused: bool,
}

fn read<T>(quantity: Option<&Quantity>, parse: impl Fn(&Quantity) -> Option<T>) -> Read<T> {
    match quantity {
        None => Read {
            value: None,
            asked_and_refused: false,
        },
        Some(quantity) => {
            let value = parse(quantity);
            Read {
                asked_and_refused: value.is_none(),
                value,
            }
        }
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

/// Ceiling on an artifact-declared VM memory size; a greedy or adversarial sandbox can't size the guest past this and starve the host (a user who genuinely needs more passes `-m` explicitly).
pub const MAX_MEM_MIB: usize = 256 * 1024;

fn quantity_to_mib(quantity: &Quantity) -> Option<usize> {
    let mib = match quantity {
        Quantity::Int(n) => usize::try_from(*n).ok(),
        Quantity::Text(text) => crate::memory::parse_mib(text).ok(),
    };
    mib.filter(|mib| (1..=MAX_MEM_MIB).contains(mib))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(
        resources: Option<&Resources>,
        overrides: &ResourceOverrides,
        defaults: VmSize,
    ) -> VmSize {
        let (declared, _) = DeclaredSize::from_resources(resources);
        resolve_declared(declared, overrides, defaults)
    }

    #[test]
    fn an_absent_sandbox_size_falls_back_to_defaults() {
        let size = resolve(None, &ResourceOverrides::default(), DEFAULT_VM_SIZE);
        assert_eq!(size, DEFAULT_VM_SIZE);
    }

    #[test]
    fn a_flag_outranks_the_definition_which_outranks_the_default() {
        let declared = DeclaredSize {
            cpus: Some(3),
            mem_mib: Some(6144),
        };
        assert_eq!(
            resolve_declared(declared, &ResourceOverrides::default(), DEFAULT_VM_SIZE),
            VmSize {
                cpus: 3,
                mem_mib: 6144
            }
        );
        assert_eq!(
            resolve_declared(
                declared,
                &ResourceOverrides {
                    cpus: Some(2),
                    mem_mib: None
                },
                DEFAULT_VM_SIZE
            ),
            VmSize {
                cpus: 2,
                mem_mib: 6144
            },
            "a flag must win without dragging the other field down to the default"
        );
        assert_eq!(
            resolve_declared(
                DeclaredSize::default(),
                &ResourceOverrides::default(),
                DEFAULT_VM_SIZE
            ),
            DEFAULT_VM_SIZE
        );
    }

    #[test]
    fn a_text_quantity_with_units_is_understood() {
        let res = Resources {
            cpu: Some(Quantity::Text("2".into())),
            memory: Some(Quantity::Text("2Gi".into())),
        };
        let size = resolve(Some(&res), &ResourceOverrides::default(), DEFAULT_VM_SIZE);
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
        let size = resolve(Some(&res), &ResourceOverrides::default(), DEFAULT_VM_SIZE);
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
            resolve(Some(&plain), &ResourceOverrides::default(), DEFAULT_VM_SIZE).mem_mib,
            640
        );
    }

    #[test]
    fn a_definition_reads_every_size_the_mem_flag_accepts() {
        for (text, mem_mib) in [
            ("38Gi", 38912),
            ("38gi", 38912),
            ("38GiB", 38912),
            ("2g", 2048),
            ("2GB", 2048),
            ("512m", 512),
            ("768Mi", 768),
            ("1024k", 1),
            ("640", 640),
        ] {
            let res = Resources {
                cpu: None,
                memory: Some(Quantity::Text(text.into())),
            };
            assert_eq!(
                resolve(Some(&res), &ResourceOverrides::default(), DEFAULT_VM_SIZE).mem_mib,
                mem_mib,
                "memory: {text}"
            );
        }
    }

    #[test]
    fn an_unparseable_or_out_of_range_quantity_falls_back_to_defaults() {
        let res = Resources {
            cpu: Some(Quantity::Int(0)),
            memory: Some(Quantity::Text("lots".into())),
        };
        let size = resolve(Some(&res), &ResourceOverrides::default(), DEFAULT_VM_SIZE);
        assert_eq!(size, DEFAULT_VM_SIZE);

        let too_many = Resources {
            cpu: Some(Quantity::Int(9000)),
            memory: Some(Quantity::Int(-4)),
        };
        assert_eq!(
            resolve(
                Some(&too_many),
                &ResourceOverrides::default(),
                DEFAULT_VM_SIZE
            ),
            DEFAULT_VM_SIZE
        );
    }

    #[test]
    fn a_memory_request_over_the_ceiling_or_that_overflows_falls_back_to_defaults() {
        let over_ceiling = Resources {
            cpu: None,
            memory: Some(Quantity::Text("999999Gi".into())),
        };
        assert_eq!(
            resolve(
                Some(&over_ceiling),
                &ResourceOverrides::default(),
                DEFAULT_VM_SIZE
            )
            .mem_mib,
            DEFAULT_VM_SIZE.mem_mib,
            "an artifact must not size the guest past the ceiling and starve the host"
        );

        let overflowing = Resources {
            cpu: None,
            memory: Some(Quantity::Text(format!("{}Gi", usize::MAX))),
        };
        assert_eq!(
            resolve(
                Some(&overflowing),
                &ResourceOverrides::default(),
                DEFAULT_VM_SIZE
            )
            .mem_mib,
            DEFAULT_VM_SIZE.mem_mib,
            "a Gi value whose *1024 overflows must fall back, not wrap to garbage"
        );

        let huge_int = Resources {
            cpu: None,
            memory: Some(Quantity::Int(i64::MAX)),
        };
        assert_eq!(
            resolve(
                Some(&huge_int),
                &ResourceOverrides::default(),
                DEFAULT_VM_SIZE
            )
            .mem_mib,
            DEFAULT_VM_SIZE.mem_mib
        );

        let at_ceiling = Resources {
            cpu: None,
            memory: Some(Quantity::Int(MAX_MEM_MIB as i64)),
        };
        assert_eq!(
            resolve(
                Some(&at_ceiling),
                &ResourceOverrides::default(),
                DEFAULT_VM_SIZE
            )
            .mem_mib,
            MAX_MEM_MIB,
            "a request exactly at the ceiling is honored"
        );
    }

    #[test]
    fn a_request_the_host_will_not_grant_is_named_so_the_caller_can_say_so() {
        let refused = Resources {
            cpu: Some(Quantity::Int(9000)),
            memory: Some(Quantity::Text("999999Gi".into())),
        };
        let (declared, ignored) = DeclaredSize::from_resources(Some(&refused));
        assert_eq!(declared, DeclaredSize::default());
        assert_eq!(ignored, vec!["cpu", "memory"]);

        let honoured = Resources {
            cpu: Some(Quantity::Int(2)),
            memory: Some(Quantity::Text("2Gi".into())),
        };
        let (declared, ignored) = DeclaredSize::from_resources(Some(&honoured));
        assert_eq!(
            declared,
            DeclaredSize {
                cpus: Some(2),
                mem_mib: Some(2048)
            }
        );
        assert!(ignored.is_empty());

        let (declared, ignored) = DeclaredSize::from_resources(None);
        assert_eq!(declared, DeclaredSize::default());
        assert!(
            ignored.is_empty(),
            "asking for nothing is not a refused request"
        );
    }
}
