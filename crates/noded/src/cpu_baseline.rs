//! The CPU baseline a node reports for its pool: the CPU features a VM's snapshot depends on, read once at
//! start. Two nodes share a pool only when their architecture, hypervisor and baseline match, so a live VM
//! moves between them (plan 17.2). The parsers are pure over the text or registers they are given; the
//! reads are thin wrappers. arm64 Linux reads `/proc/cpuinfo` (the features every core has, and each core's
//! MIDR) and the auxiliary vector (AT_HWCAP, AT_HWCAP2); x86_64 reads CPUID, without the APIC ID that
//! differs per core; Apple Silicon reads the chip family. Elsewhere there is no baseline (`None`), and a
//! node with none is in no pool.

use std::collections::BTreeSet;

/// The auxiliary vector's key for the arm64 hardware capability word.
const AT_HWCAP: u64 = 16;
/// The auxiliary vector's key for the second arm64 hardware capability word.
const AT_HWCAP2: u64 = 26;

/// The CPUID leaves a baseline reads, each as `[eax, ebx, ecx, edx]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cpuid {
    /// Leaf 0: the highest basic leaf, and the vendor string spread over ebx, edx and ecx.
    pub leaf0: [u32; 4],
    /// Leaf 1: the signature (family, model, stepping) and the feature flags. Its ebx is the APIC ID and
    /// the logical processor count, which differ per core, so the baseline leaves it out.
    pub leaf1: [u32; 4],
    /// Leaf 7, subleaf 0: the extended feature flags.
    pub leaf7: [u32; 4],
    /// Leaf 0xD, subleaf 0: the XSAVE state components the CPU supports (eax) and the size of the area for
    /// those the OS enabled (ebx); a snapshot's register state is laid out by them.
    pub xsave: [u32; 4],
    /// Leaf 0x80000001: the extended feature flags (ecx and edx).
    pub ext1: [u32; 4],
}

/// The x86_64 baseline (pure): the vendor, the signature and the feature words of leaves 1, 7, 0xD and
/// 0x80000001. Two cores of one CPU give the same text.
pub fn x86_baseline(cpuid: &Cpuid) -> String {
    let [_, b0, c0, d0] = cpuid.leaf0;
    let vendor: Vec<u8> = [b0, d0, c0]
        .iter()
        .flat_map(|reg| reg.to_le_bytes())
        .collect();
    let vendor = String::from_utf8_lossy(&vendor);
    let [sig, _, f1c, f1d] = cpuid.leaf1;
    let [_, b7, c7, d7] = cpuid.leaf7;
    let [xs, xb, _, _] = cpuid.xsave;
    let [_, _, e1c, e1d] = cpuid.ext1;
    format!(
        "x86_64 vendor={} sig={sig:#010x} leaf1={f1c:#010x},{f1d:#010x} leaf7={b7:#010x},{c7:#010x},{d7:#010x} xsave={xs:#x},{xb:#x} ext1={e1c:#010x},{e1d:#010x}",
        vendor.trim_end_matches('\0')
    )
}

/// The arm64 baseline (pure): each core's MIDR (`implementer:part:variant:revision`, as cpuinfo writes
/// them), the features every core has, and the auxiliary vector's two hardware capability words. None
/// when no core block has all of these.
pub fn arm64_baseline(cpuinfo: &str, auxv: &[u8]) -> Option<String> {
    let mut midrs = BTreeSet::new();
    let mut features: Option<BTreeSet<&str>> = None;
    for core in cpuinfo
        .split("\n\n")
        .filter(|block| block.lines().any(|line| line.starts_with("processor")))
    {
        let field = |key: &str| {
            core.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                (name.trim() == key).then(|| value.trim())
            })
        };
        let midr = [
            field("CPU implementer")?,
            field("CPU part")?,
            field("CPU variant")?,
            field("CPU revision")?,
        ]
        .join(":");
        midrs.insert(midr);
        let core_features: BTreeSet<&str> = field("Features")?.split_whitespace().collect();
        features = Some(match features {
            None => core_features,
            Some(common) => common.intersection(&core_features).copied().collect(),
        });
    }
    let features = features?;
    let hwcap = auxv_word(auxv, AT_HWCAP).unwrap_or(0);
    let hwcap2 = auxv_word(auxv, AT_HWCAP2).unwrap_or(0);
    let midrs: Vec<&str> = midrs.iter().map(String::as_str).collect();
    let features: Vec<&str> = features.into_iter().collect();
    Some(format!(
        "arm64 midr={} features={} hwcap={hwcap:#x},{hwcap2:#x}",
        midrs.join(","),
        features.join(",")
    ))
}

/// The value of auxiliary vector entry `key` (pure): the vector is `(key, value)` pairs of 64-bit words,
/// little-endian as arm64 Linux writes them, ended by a zero key.
pub fn auxv_word(auxv: &[u8], key: u64) -> Option<u64> {
    for pair in auxv.as_chunks::<16>().0 {
        let entry = u64::from_le_bytes(pair[..8].try_into().ok()?);
        let value = u64::from_le_bytes(pair[8..].try_into().ok()?);
        match entry {
            0 => return None,
            found if found == key => return Some(value),
            _ => {}
        }
    }
    None
}

/// The arm64 Apple Silicon baseline (pure) from the chip family sysctl's `hw.cpufamily` (a decimal number).
pub fn apple_baseline(cpufamily: &str) -> Option<String> {
    let family: u64 = cpufamily.trim().parse().ok()?;
    Some(format!("arm64 cpufamily={family:#x}"))
}

/// The baseline of this host, or None where the platform has none to read.
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
pub fn host() -> Option<String> {
    let cpuinfo = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let auxv = std::fs::read("/proc/self/auxv").ok()?;
    arm64_baseline(&cpuinfo, &auxv)
}

/// The baseline of this host, or None where the platform has none to read.
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
pub fn host() -> Option<String> {
    apple_baseline(&crate::sysctl("hw.cpufamily")?)
}

/// The baseline of this host, or None where the platform has none to read.
#[cfg(target_arch = "x86_64")]
pub fn host() -> Option<String> {
    Some(x86_baseline(&read_cpuid()))
}

/// The baseline of this host, or None where the platform has none to read.
#[cfg(not(any(
    all(target_os = "linux", target_arch = "aarch64"),
    all(target_os = "macos", target_arch = "aarch64"),
    target_arch = "x86_64"
)))]
pub fn host() -> Option<String> {
    None
}

/// The CPUID leaves of this CPU (the core that runs the call). A leaf past the CPU's maximum reads as
/// the highest basic leaf, which is how CPUID itself answers.
#[cfg(target_arch = "x86_64")]
fn read_cpuid() -> Cpuid {
    use std::arch::x86_64::__cpuid_count;
    let cpuid = |leaf: u32, sub: u32| {
        let regs = __cpuid_count(leaf, sub);
        [regs.eax, regs.ebx, regs.ecx, regs.edx]
    };
    Cpuid {
        leaf0: cpuid(0, 0),
        leaf1: cpuid(1, 0),
        leaf7: cpuid(7, 0),
        xsave: cpuid(0xD, 0),
        ext1: cpuid(0x8000_0001, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An auxiliary vector of the given `(key, value)` pairs, ended by a zero key.
    fn auxv(pairs: &[(u64, u64)]) -> Vec<u8> {
        let mut bytes: Vec<u8> = pairs
            .iter()
            .flat_map(|(key, value)| key.to_le_bytes().into_iter().chain(value.to_le_bytes()))
            .collect();
        bytes.extend([0u8; 16]);
        bytes
    }

    /// One arm64 core's cpuinfo block.
    fn core(index: u32, features: &str, part: &str) -> String {
        format!(
            "processor\t: {index}\nBogoMIPS\t: 50.00\nFeatures\t: {features}\nCPU implementer\t: 0x41\nCPU architecture: 8\nCPU variant\t: 0x3\nCPU part\t: {part}\nCPU revision\t: 1\n"
        )
    }

    #[test]
    fn an_arm64_baseline_is_the_common_features_and_each_midr() {
        let cpuinfo = format!(
            "{}\n{}\n{}",
            core(0, "fp asimd aes crc32", "0xd0c"),
            core(1, "fp asimd aes", "0xd0c"),
            core(2, "fp asimd crc32", "0xd0d")
        );
        let baseline = arm64_baseline(&cpuinfo, &auxv(&[(AT_HWCAP, 0x2), (AT_HWCAP2, 0x1)]));
        assert_eq!(
            baseline.as_deref(),
            Some("arm64 midr=0x41:0xd0c:0x3:1,0x41:0xd0d:0x3:1 features=asimd,fp hwcap=0x2,0x1"),
            "the features every core has, and each distinct MIDR once"
        );
    }

    #[test]
    fn an_arm64_baseline_needs_every_core_to_say_what_it_is() {
        let vector = auxv(&[(AT_HWCAP, 0x2)]);
        let without_part = core(0, "fp asimd", "0xd0c").replace("CPU part", "CPU parts");
        for (case, cpuinfo) in [
            ("no core block at all", String::from("Hardware\t: none\n")),
            ("a core without its MIDR", without_part),
            (
                "a core without its features",
                core(0, "fp asimd", "0xd0c").replace("Features", "Flags"),
            ),
        ] {
            assert_eq!(arm64_baseline(&cpuinfo, &vector), None, "{case}");
        }
    }

    #[test]
    fn the_auxiliary_vector_gives_the_hardware_words_or_none() {
        let vector = auxv(&[(3, 0x40), (AT_HWCAP, 0xdead_beef), (AT_HWCAP2, 0x7)]);
        assert_eq!(auxv_word(&vector, AT_HWCAP), Some(0xdead_beef));
        assert_eq!(auxv_word(&vector, AT_HWCAP2), Some(0x7));
        assert_eq!(auxv_word(&vector, 99), None, "an absent key");
        assert_eq!(auxv_word(&auxv(&[]), AT_HWCAP), None, "an empty vector");
    }

    #[test]
    fn an_apple_baseline_is_the_chip_family() {
        assert_eq!(
            apple_baseline("1744204979").as_deref(),
            Some("arm64 cpufamily=0x67f674b3")
        );
        assert_eq!(apple_baseline("not a number"), None);
    }

    /// Recorded-style CPUID values of one Intel core. The sample values are not from a particular CPU.
    fn intel_core(apic: u32) -> Cpuid {
        Cpuid {
            leaf0: [
                0x20,
                u32::from_le_bytes(*b"Genu"),
                u32::from_le_bytes(*b"ntel"),
                u32::from_le_bytes(*b"ineI"),
            ],
            // Stepping 4 of model 0x8e: the signature, then the APIC ID in ebx[31:24].
            leaf1: [
                0x0008_06e9,
                (apic << 24) | 0x0001_0800,
                0x7ffa_3203,
                0xbfeb_fbff,
            ],
            leaf7: [0, 0x0000_9c27, 0x0040_0000, 0xfc00_0400],
            xsave: [0x0000_0007, 0x0000_0340, 0, 0],
            ext1: [0, 0, 0x0000_0121, 0x2c10_0800],
        }
    }

    #[test]
    fn an_x86_baseline_names_the_vendor_and_the_feature_words() {
        assert_eq!(
            x86_baseline(&intel_core(0)),
            "x86_64 vendor=GenuineIntel sig=0x000806e9 leaf1=0x7ffa3203,0xbfebfbff leaf7=0x00009c27,0x00400000,0xfc000400 xsave=0x7,0x340 ext1=0x00000121,0x2c100800"
        );
    }

    #[test]
    fn an_x86_baseline_is_the_same_on_every_core_and_changes_with_the_cpu() {
        assert_eq!(
            x86_baseline(&intel_core(0)),
            x86_baseline(&intel_core(5)),
            "the APIC ID is per core, not part of the baseline"
        );
        let mut stepping = intel_core(0);
        stepping.leaf1[0] += 1;
        let mut feature = intel_core(0);
        feature.leaf7[2] ^= 1 << 22;
        for (case, other) in [("a stepping", stepping), ("a feature flag", feature)] {
            assert_ne!(
                x86_baseline(&intel_core(0)),
                x86_baseline(&other),
                "{case} is a new baseline"
            );
        }
    }

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    #[test]
    fn the_container_reads_its_own_arm64_baseline() {
        let baseline = host().expect("an arm64 Linux host has a baseline");
        assert!(baseline.starts_with("arm64 midr=0x"), "{baseline}");
        assert!(
            baseline.contains(" features=") && baseline.contains("asimd"),
            "{baseline}"
        );
        let hwcap = auxv_word(&std::fs::read("/proc/self/auxv").expect("auxv"), AT_HWCAP);
        assert!(
            hwcap.is_some_and(|word| word & (1 << 1) != 0),
            "arm64 Linux always has ASIMD: {baseline}"
        );
    }

    #[cfg(target_arch = "x86_64")]
    #[test]
    fn this_x86_cpu_reads_a_vendor() {
        let baseline = host().expect("an x86_64 host has a baseline");
        assert!(baseline.starts_with("x86_64 vendor="), "{baseline}");
    }
}
