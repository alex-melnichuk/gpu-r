//! Windows device enumeration.
//!
//! There is no sysfs on Windows, so the PCI identity comes from the PnP
//! instance ID (`PCI\VEN_1002&DEV_73BF&SUBSYS_52021849&REV_C1\...`), the bus
//! location from the PnP device properties and the driver-reported VRAM size
//! from the display class registry key. One PowerShell call collects all of it.

use crate::pci::PciDevice;

const SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$r = @()
foreach ($v in Get-CimInstance Win32_VideoController) {
  $id = $v.PNPDeviceID
  if (-not $id -or -not $id.StartsWith('PCI\')) { continue }
  $loc = (Get-PnpDeviceProperty -InstanceId $id -KeyName DEVPKEY_Device_LocationInfo).Data
  $svc = (Get-PnpDeviceProperty -InstanceId $id -KeyName DEVPKEY_Device_Service).Data
  $key = (Get-PnpDeviceProperty -InstanceId $id -KeyName DEVPKEY_Device_Driver).Data
  $mem = $null
  if ($key) {
    $p = Get-ItemProperty "HKLM:\SYSTEM\CurrentControlSet\Control\Class\$key"
    $mem = $p.'HardwareInformation.qwMemorySize'
    if (-not $mem) {
      $m = $p.'HardwareInformation.MemorySize'
      if ($m -is [byte[]]) { $mem = [BitConverter]::ToUInt32($m, 0) } else { $mem = $m }
    }
  }
  $date = $null
  if ($v.DriverDate) { $date = $v.DriverDate.ToString('yyyy-MM-dd') }
  $r += [pscustomobject]@{ name = $v.Name; pnp = $id; ver = $v.DriverVersion; date = $date; loc = $loc; svc = $svc; mem = [string]$mem }
}
ConvertTo-Json -InputObject $r -Compress
"#;

pub fn enumerate() -> Vec<PciDevice> {
    let v = enumerate_pnp();
    if !v.is_empty() {
        return v;
    }
    // No PowerShell/WMI (restricted shell, Wine): fall back to the graphics API adapters.
    let mut out: Vec<PciDevice> = vec![];
    for a in crate::gpu::enumerate_vulkan() {
        let (vendor, device) = (a.vendor as u16, a.device as u16);
        if vendor == 0 || out.iter().any(|d| d.vendor == vendor && d.device == device) {
            continue;
        }
        out.push(PciDevice {
            addr: format!("adapter {}", out.len()),
            vendor,
            device,
            class: 0x030000,
            driver: Some(match vendor { 0x10DE => "nvidia", 0x1002 => "amd", 0x8086 => "intel", _ => "unknown" }.to_string()),
            device_name: Some(a.name.clone()),
            os_driver_version: Some(a.driver_info.clone()).filter(|s| !s.is_empty()),
            ..Default::default()
        });
    }
    out
}

fn enumerate_pnp() -> Vec<PciDevice> {
    let Some(out) = crate::util::run("powershell", &["-NoProfile", "-NonInteractive", "-Command", SCRIPT]) else {
        return vec![];
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&out) else {
        return vec![];
    };
    let items = match v {
        serde_json::Value::Array(a) => a,
        o @ serde_json::Value::Object(_) => vec![o],
        _ => vec![],
    };
    items.iter().filter_map(parse).collect()
}

fn hex_after(s: &str, key: &str, len: usize) -> Option<u32> {
    let i = s.find(key)? + key.len();
    u32::from_str_radix(s.get(i..i + len)?, 16).ok()
}

fn parse(v: &serde_json::Value) -> Option<PciDevice> {
    let str_of = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string).filter(|s| !s.is_empty());
    let pnp = str_of("pnp")?.to_uppercase();
    let vendor = hex_after(&pnp, "VEN_", 4)? as u16;
    let device = hex_after(&pnp, "DEV_", 4)? as u16;
    let subsys = hex_after(&pnp, "SUBSYS_", 8).unwrap_or(0);
    let rev = hex_after(&pnp, "REV_", 2);
    let revision = rev.unwrap_or(0) as u8;
    // "PCI bus 3, device 0, function 0"
    let addr = str_of("loc")
        .and_then(|l| {
            let n: Vec<u32> = l.split(|c: char| !c.is_ascii_digit()).filter_map(|x| x.parse().ok()).collect();
            (n.len() >= 3).then(|| format!("0000:{:02x}:{:02x}.{}", n[0], n[1], n[2]))
        })
        .unwrap_or_else(|| pnp.clone());
    let driver = str_of("svc").map(|s| match s.to_lowercase().as_str() {
        "nvlddmkm" => "nvidia".to_string(),
        "amdkmdag" | "amdwddmg" => "amd".to_string(),
        "igfx" | "igfxn" | "igfxcui" => "intel".to_string(),
        o => o.to_string(),
    });
    Some(PciDevice {
        addr,
        vendor,
        device,
        subsys_vendor: (subsys & 0xffff) as u16,
        subsys_device: (subsys >> 16) as u16,
        revision,
        rev_known: rev.is_some(),
        class: 0x030000,
        driver,
        device_name: str_of("name"),
        os_driver_version: str_of("ver"),
        os_driver_date: str_of("date"),
        os_vram: str_of("mem").and_then(|m| m.parse().ok()).filter(|&m: &u64| m > 0),
        ..Default::default()
    })
}
