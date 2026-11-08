//! 平台检测：目标操作系统 / 架构 / 主机，以及跨平台命名规则。

use std::fmt;

/// 目标操作系统。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Linux,
    Macos,
    Windows,
    Freebsd,
    Netbsd,
    Openbsd,
    Dragonfly,
    Android,
    Ios,
    Unknown,
}

impl Os {
    /// 当前运行的操作系统。
    pub fn current() -> Self {
        Self::parse(std::env::consts::OS)
    }

    /// 从字符串解析（未知回退 Unknown）。
    pub fn parse(s: &str) -> Self {
        match s {
            "linux" => Self::Linux,
            "macos" => Self::Macos,
            "windows" => Self::Windows,
            "freebsd" => Self::Freebsd,
            "netbsd" => Self::Netbsd,
            "openbsd" => Self::Openbsd,
            "dragonfly" => Self::Dragonfly,
            "android" => Self::Android,
            "ios" => Self::Ios,
            _ => Self::Unknown,
        }
    }

    /// 是否 Windows。
    ///
    /// 只有 Windows 才用 `cmd /C`、`.exe` 后缀、无 rpath 等规则；
    /// Android/iOS 是 Unix-like（.so/.dylib、`@rpath`、Unix 链接器旗标）。
    pub fn is_windows_family(&self) -> bool {
        matches!(self, Self::Windows)
    }

    /// 动态库文件名，如 `libfoo.dylib` / `libfoo.so` / `foo.dll`。
    pub fn dylib_name(&self, base: &str) -> String {
        match self {
            Self::Windows => format!("{}.dll", base),
            Self::Macos | Self::Ios => format!("lib{}.dylib", base),
            _ => format!("lib{}.so", base),
        }
    }

    /// 带 soversion 的动态库文件名：`libfoo.so.N`（Linux）/ `libfoo.N.dylib`（macOS）。
    pub fn dylib_name_versioned(&self, base: &str, version: &str) -> String {
        match self {
            Self::Macos | Self::Ios => format!("lib{}.{}.dylib", base, version),
            _ => format!("lib{}.so.{}", base, version),
        }
    }

    /// 开发者符号链接名（去掉版本）：`libfoo.so` / `libfoo.dylib`。
    pub fn dylib_dev_link(&self, base: &str) -> String {
        match self {
            Self::Windows => format!("{}.dll", base),
            Self::Macos | Self::Ios => format!("lib{}.dylib", base),
            _ => format!("lib{}.so", base),
        }
    }

    /// 从实际库文件名推出开发者符号链接名：
    /// `libfoo.so.1` → `libfoo.so`，`libfoo.1.dylib` → `libfoo.dylib`。
    pub fn dev_link_name(file_name: &str) -> Option<String> {
        if let Some(idx) = file_name.find(".so") {
            return Some(file_name[..idx + 3].to_string());
        }
        if let Some(stem) = file_name.strip_suffix(".dylib") {
            let dev = match stem.rfind('.') {
                Some(i) => &stem[..i],
                None => stem,
            };
            return Some(format!("{dev}.dylib"));
        }
        None
    }

    /// 静态库文件名，如 `libfoo.a`。
    pub fn static_name(&self, base: &str) -> String {
        format!("lib{}.a", base)
    }

    /// 可执行文件后缀。
    pub fn exe_suffix(&self) -> &'static str {
        if self.is_windows_family() {
            ".exe"
        } else {
            ""
        }
    }
}

impl fmt::Display for Os {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Linux => "linux",
            Self::Macos => "macos",
            Self::Windows => "windows",
            Self::Freebsd => "freebsd",
            Self::Netbsd => "netbsd",
            Self::Openbsd => "openbsd",
            Self::Dragonfly => "dragonfly",
            Self::Android => "android",
            Self::Ios => "ios",
            Self::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

/// 目标架构。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Arch {
    X86_64,
    X86,
    Aarch64,
    Arm,
    Riscv64,
    Powerpc64,
    Wasm32,
    Unknown,
}

impl Arch {
    /// 当前运行架构。
    pub fn current() -> Self {
        Self::parse(std::env::consts::ARCH)
    }

    /// 从字符串解析（未知回退 Unknown）。
    pub fn parse(s: &str) -> Self {
        match s {
            "x86_64" | "amd64" => Self::X86_64,
            "x86" | "i386" | "i686" => Self::X86,
            "aarch64" | "arm64" => Self::Aarch64,
            "arm" => Self::Arm,
            "riscv64" => Self::Riscv64,
            "powerpc64" => Self::Powerpc64,
            "wasm32" => Self::Wasm32,
            _ => Self::Unknown,
        }
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::X86_64 => "x86_64",
            Self::X86 => "x86",
            Self::Aarch64 => "aarch64",
            Self::Arm => "arm",
            Self::Riscv64 => "riscv64",
            Self::Powerpc64 => "powerpc64",
            Self::Wasm32 => "wasm32",
            Self::Unknown => "unknown",
        };
        f.write_str(s)
    }
}

/// 完整主机：OS + 架构。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Host {
    pub os: Os,
    pub arch: Arch,
}

impl Host {
    /// 当前主机。
    pub fn current() -> Self {
        Self {
            os: Os::current(),
            arch: Arch::current(),
        }
    }
}

impl fmt::Display for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.arch, self.os)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn naming_rules() {
        assert_eq!(Os::Macos.dylib_name("foo"), "libfoo.dylib");
        assert_eq!(Os::Linux.dylib_name("foo"), "libfoo.so");
        assert_eq!(Os::Windows.dylib_name("foo"), "foo.dll");
        assert_eq!(Os::Linux.static_name("foo"), "libfoo.a");
        assert_eq!(Os::Macos.exe_suffix(), "");
        assert_eq!(Os::Windows.exe_suffix(), ".exe");
        // Android/iOS 是 Unix-like：无 .exe 后缀、走 Unix 链接器规则。
        assert_eq!(Os::Android.exe_suffix(), "");
        assert_eq!(Os::Ios.exe_suffix(), "");
        assert!(!Os::Ios.is_windows_family());
        assert!(!Os::Android.is_windows_family());
        assert!(Os::Windows.is_windows_family());
    }

    #[test]
    fn arch_aliases() {
        assert_eq!(Arch::parse("amd64"), Arch::X86_64);
        assert_eq!(Arch::parse("arm64"), Arch::Aarch64);
        assert_eq!(Arch::parse("i686"), Arch::X86);
    }
}
