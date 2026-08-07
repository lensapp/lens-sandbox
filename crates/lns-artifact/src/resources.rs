pub mod host;

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

/// What this machine has in total — not what is free — so a definition sizing itself as a share of it lands the same on every run of one host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostCapacity {
    pub cpus: u8,
    pub mem_mib: usize,
}

impl DeclaredSize {
    /// The `ignored` field names each request the host will not grant, so the caller that owns a log can say so.
    pub fn from_resources(
        resources: Option<&Resources>,
        host: Option<HostCapacity>,
    ) -> (Self, Vec<&'static str>) {
        let mut ignored = Vec::new();
        let cpus = read(resources.and_then(|r| r.cpu.as_ref()), |q| {
            quantity_to_cpus(q, host)
        });
        let mem_mib = read(resources.and_then(|r| r.memory.as_ref()), |q| {
            quantity_to_mib(q, host)
        });
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

fn quantity_to_cpus(quantity: &Quantity, host: Option<HostCapacity>) -> Option<u8> {
    let whole = match quantity {
        Quantity::Int(n) => u32::try_from(*n).ok()?,
        Quantity::Text(text) => match parse_percent(text) {
            // A share can only ever raise the guest to the floor, never lower it below what boots.
            Some(pct) => u32::from(cpu_share(host?.cpus, pct).max(DEFAULT_VM_SIZE.cpus)),
            None => parse_cpu_text(text)?,
        },
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

/// A whole-number percentage of this host, 1 through 100; anything else is not a share.
pub(crate) fn parse_percent(text: &str) -> Option<u8> {
    let digits = text.trim().strip_suffix('%')?;
    digits
        .parse::<u8>()
        .ok()
        .filter(|pct| (1..=100).contains(pct))
}

/// Both shares are computed in a wider type and are never larger than the total, so neither narrowing can wrap.
fn cpu_share(total: u8, pct: u8) -> u8 {
    (u32::from(total) * u32::from(pct) / 100) as u8
}

fn mem_share(total: usize, pct: u8) -> usize {
    (total as u128 * u128::from(pct) / 100) as usize
}

/// Ceiling on an artifact-declared VM memory size; a greedy or adversarial sandbox can't size the guest past this and starve the host (a user who genuinely needs more passes `-m` explicitly).
pub const MAX_MEM_MIB: usize = 256 * 1024;

fn quantity_to_mib(quantity: &Quantity, host: Option<HostCapacity>) -> Option<usize> {
    // A share is clamped into what boots, because the author asked for "some of this host" and any host can answer that; an absolute size out of range is a request this host refuses.
    let in_range = |mib: &usize| (1..=MAX_MEM_MIB).contains(mib);
    match quantity {
        Quantity::Int(n) => usize::try_from(*n).ok().filter(in_range),
        Quantity::Text(text) => match parse_percent(text) {
            Some(pct) => {
                Some(mem_share(host?.mem_mib, pct).clamp(DEFAULT_VM_SIZE.mem_mib, MAX_MEM_MIB))
            }
            None => crate::memory::parse_mib(text).ok().filter(in_range),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolve(
        resources: Option<&Resources>,
        overrides: &ResourceOverrides,
        defaults: VmSize,
    ) -> VmSize {
        let (declared, _) = DeclaredSize::from_resources(resources, None);
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

    const TEN_CORE_16G: HostCapacity = HostCapacity {
        cpus: 10,
        mem_mib: 16384,
    };

    #[test]
    fn a_definition_sized_in_percent_takes_that_share_of_this_host() {
        let res = Resources {
            cpu: Some(Quantity::Text("80%".into())),
            memory: Some(Quantity::Text("80%".into())),
        };
        let (declared, ignored) = DeclaredSize::from_resources(Some(&res), Some(TEN_CORE_16G));

        assert_eq!(
            declared,
            DeclaredSize {
                cpus: Some(8),
                mem_mib: Some(13107)
            }
        );
        assert!(ignored.is_empty());
    }

    #[test]
    fn a_share_too_small_to_boot_is_lifted_to_the_built_in_floor() {
        let tiny = HostCapacity {
            cpus: 1,
            mem_mib: 512,
        };
        let res = Resources {
            cpu: Some(Quantity::Text("80%".into())),
            memory: Some(Quantity::Text("80%".into())),
        };
        let (declared, ignored) = DeclaredSize::from_resources(Some(&res), Some(tiny));

        assert_eq!(
            declared,
            DeclaredSize {
                cpus: Some(DEFAULT_VM_SIZE.cpus),
                mem_mib: Some(DEFAULT_VM_SIZE.mem_mib)
            },
            "a share is a request, and must never leave the guest too small to boot"
        );
        assert!(
            ignored.is_empty(),
            "the floor is not a refusal: {ignored:?}"
        );
    }

    #[test]
    fn a_full_share_still_respects_the_host_starvation_ceiling() {
        let huge = HostCapacity {
            cpus: 200,
            mem_mib: 512 * 1024,
        };
        let res = Resources {
            cpu: None,
            memory: Some(Quantity::Text("100%".into())),
        };
        let (declared, _) = DeclaredSize::from_resources(Some(&res), Some(huge));

        assert_eq!(
            declared.mem_mib,
            Some(MAX_MEM_MIB),
            "a percentage must not walk past the ceiling an absolute size cannot"
        );
    }

    #[test]
    fn a_percentage_with_no_host_reading_falls_back_rather_than_guessing() {
        let res = Resources {
            cpu: Some(Quantity::Text("80%".into())),
            memory: Some(Quantity::Text("80%".into())),
        };
        let (declared, ignored) = DeclaredSize::from_resources(Some(&res), None);

        assert_eq!(declared, DeclaredSize::default());
        assert_eq!(
            ignored,
            vec!["cpu", "memory"],
            "an unresolvable share must be reported, not silently booted at some other size"
        );
    }

    #[test]
    fn a_percentage_outside_one_to_a_hundred_is_refused() {
        for pct in ["0%", "101%", "%", "8 0%", "-5%", "1000%"] {
            let res = Resources {
                cpu: Some(Quantity::Text(pct.into())),
                memory: Some(Quantity::Text(pct.into())),
            };
            let (declared, ignored) = DeclaredSize::from_resources(Some(&res), Some(TEN_CORE_16G));
            assert_eq!(declared, DeclaredSize::default(), "pct {pct}");
            assert_eq!(ignored, vec!["cpu", "memory"], "pct {pct}");
        }
    }

    #[test]
    fn a_request_the_host_will_not_grant_is_named_so_the_caller_can_say_so() {
        let refused = Resources {
            cpu: Some(Quantity::Int(9000)),
            memory: Some(Quantity::Text("999999Gi".into())),
        };
        let (declared, ignored) = DeclaredSize::from_resources(Some(&refused), None);
        assert_eq!(declared, DeclaredSize::default());
        assert_eq!(ignored, vec!["cpu", "memory"]);

        let honoured = Resources {
            cpu: Some(Quantity::Int(2)),
            memory: Some(Quantity::Text("2Gi".into())),
        };
        let (declared, ignored) = DeclaredSize::from_resources(Some(&honoured), None);
        assert_eq!(
            declared,
            DeclaredSize {
                cpus: Some(2),
                mem_mib: Some(2048)
            }
        );
        assert!(ignored.is_empty());

        let (declared, ignored) = DeclaredSize::from_resources(None, None);
        assert_eq!(declared, DeclaredSize::default());
        assert!(
            ignored.is_empty(),
            "asking for nothing is not a refused request"
        );
    }
}
