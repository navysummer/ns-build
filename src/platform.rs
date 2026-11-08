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
        Self::from_str(std::env::consts::OS)
    }

    pub fn from_str(s: &str) -> Self {
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

    /// 是否 Windows 家族。
    pub fn is_windows_family(&self) -> bool {
        matches!(self, Self::Windows | Self::Android | Self::Ios)
    }

    /// 动态库文件名，如 `libfoo.dylib` / `libfoo.so` / `foo.dll`。
    pub fn dylib_name(&self, base: &str) -> String {
        match self {
            Self::Windows => format!("{}.dll", base),
            Self::Macos | Self::Ios => format!("lib{}.dylib", base),
            _ => format!("lib{}.so", base),
        }
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
        Self::from_str(std::env::consts::ARCH)
    }

    pub fn from_str(s: &str) -> Self {
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
    }

    #[test]
    fn arch_aliases() {
        assert_eq!(Arch::from_str("amd64"), Arch::X86_64);
        assert_eq!(Arch::from_str("arm64"), Arch::Aarch64);
        assert_eq!(Arch::from_str("i686"), Arch::X86);
    }
}
