#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CpuTier {
    Scalar,
    #[cfg(target_arch = "x86_64")]
    Bmi2,
    #[cfg(target_arch = "x86_64")]
    Avx2,
}

#[cfg(feature = "std")]
static CPU_TIER: std::sync::OnceLock<CpuTier> = std::sync::OnceLock::new();

#[cfg(feature = "std")]
pub fn cpu_tier() -> CpuTier {
    if cfg!(miri) {
        return CpuTier::Scalar;
    }
    *CPU_TIER.get_or_init(detect_cpu_tier)
}

#[cfg(not(feature = "std"))]
pub fn cpu_tier() -> CpuTier {
    compile_time_tier()
}

#[cfg(feature = "std")]
fn detect_cpu_tier() -> CpuTier {
    #[cfg(target_arch = "x86_64")]
    {
        if std::arch::is_x86_feature_detected!("avx2")
            && std::arch::is_x86_feature_detected!("bmi2")
        {
            return CpuTier::Avx2;
        }
        if std::arch::is_x86_feature_detected!("bmi2") {
            return CpuTier::Bmi2;
        }
        CpuTier::Scalar
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        CpuTier::Scalar
    }
}

#[cfg(not(feature = "std"))]
fn compile_time_tier() -> CpuTier {
    #[cfg(target_arch = "x86_64")]
    {
        if cfg!(target_feature = "avx2") && cfg!(target_feature = "bmi2") {
            CpuTier::Avx2
        } else if cfg!(target_feature = "bmi2") {
            CpuTier::Bmi2
        } else {
            CpuTier::Scalar
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        CpuTier::Scalar
    }
}
