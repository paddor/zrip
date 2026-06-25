pub mod scalar;

#[cfg(all(target_arch = "x86_64", not(feature = "paranoid")))]
pub mod x86_64;

#[cfg(all(target_arch = "aarch64", not(feature = "paranoid")))]
pub mod aarch64;

#[cfg(all(target_arch = "wasm32", not(feature = "paranoid")))]
pub mod wasm32;

pub mod copy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CpuTier {
    Scalar,
    #[cfg(target_arch = "x86_64")]
    Sse2,
    #[cfg(target_arch = "x86_64")]
    Bmi2,
    #[cfg(target_arch = "x86_64")]
    Avx2,
    #[cfg(target_arch = "aarch64")]
    Neon,
    #[cfg(target_arch = "wasm32")]
    Wasm32Simd128,
}

#[cfg(all(feature = "std", not(feature = "paranoid")))]
static CPU_TIER: std::sync::OnceLock<CpuTier> = std::sync::OnceLock::new();

#[cfg(feature = "paranoid")]
pub fn cpu_tier() -> CpuTier {
    CpuTier::Scalar
}

#[cfg(all(feature = "std", not(feature = "paranoid")))]
pub fn cpu_tier() -> CpuTier {
    if cfg!(miri) {
        return CpuTier::Scalar;
    }
    *CPU_TIER.get_or_init(detect_cpu_tier)
}

#[cfg(all(not(feature = "std"), not(feature = "paranoid")))]
pub fn cpu_tier() -> CpuTier {
    compile_time_tier()
}

#[cfg(all(feature = "std", not(feature = "paranoid")))]
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
        if std::arch::is_x86_feature_detected!("sse2") {
            return CpuTier::Sse2;
        }
        CpuTier::Scalar
    }
    #[cfg(target_arch = "aarch64")]
    {
        CpuTier::Neon
    }
    #[cfg(target_arch = "wasm32")]
    {
        if cfg!(target_feature = "simd128") {
            CpuTier::Wasm32Simd128
        } else {
            CpuTier::Scalar
        }
    }
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "wasm32"
    )))]
    {
        CpuTier::Scalar
    }
}

#[cfg(all(not(feature = "std"), not(feature = "paranoid")))]
fn compile_time_tier() -> CpuTier {
    #[cfg(target_arch = "x86_64")]
    {
        if cfg!(target_feature = "avx2") && cfg!(target_feature = "bmi2") {
            CpuTier::Avx2
        } else if cfg!(target_feature = "bmi2") {
            CpuTier::Bmi2
        } else if cfg!(target_feature = "sse2") {
            CpuTier::Sse2
        } else {
            CpuTier::Scalar
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        CpuTier::Neon
    }
    #[cfg(target_arch = "wasm32")]
    {
        if cfg!(target_feature = "simd128") {
            CpuTier::Wasm32Simd128
        } else {
            CpuTier::Scalar
        }
    }
    #[cfg(not(any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "wasm32"
    )))]
    {
        CpuTier::Scalar
    }
}
