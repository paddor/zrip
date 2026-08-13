#[cfg(feature = "std")]
static HAS_BMI2: std::sync::OnceLock<bool> = std::sync::OnceLock::new();

#[cfg(feature = "std")]
pub fn has_bmi2() -> bool {
    if cfg!(miri) {
        return false;
    }
    *HAS_BMI2.get_or_init(detect_bmi2)
}

#[cfg(not(feature = "std"))]
pub fn has_bmi2() -> bool {
    compile_time_bmi2()
}

#[cfg(feature = "std")]
fn detect_bmi2() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("bmi2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

#[cfg(not(feature = "std"))]
fn compile_time_bmi2() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        cfg!(target_feature = "bmi2")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}
