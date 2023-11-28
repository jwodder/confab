cfg_if::cfg_if! {
    if #[cfg(feature = "rustls")] {
        mod rustls;
        pub(crate) use self::rustls::*;
    } else if #[cfg(feature = "native")] {
        mod native_tls;
        pub(crate) use self::native_tls::*;
    } else {
        compile_error("confab requires feature \"rustls\" or \"native\" to be enabled")
    }
}
