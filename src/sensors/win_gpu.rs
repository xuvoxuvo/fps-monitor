//! Vendor-neutral GPU readings straight from Windows.
//!
//! No vendor SDK, no helper process and no kernel driver are needed:
//!
//! * temperature: `D3DKMTQueryAdapterInfo(ADAPTER_PERFDATA)` from the graphics
//!   kernel (the same data behind fan/temperature tooling)
//! * utilisation: the `GPU Engine` performance counters, busiest engine per
//!   adapter, the way Task Manager reports it
//!
//! Everything is best effort: failures leave `None`.

use std::ffi::c_void;

use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW,
    PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY,
    PDH_MORE_DATA,
};

const PERFDATA_TYPE: i32 = 62; // KMTQAITYPE_ADAPTERPERFDATA
const PERFDATA_SIZE: u32 = 64;
const TEMP_OFFSET: usize = 56; // ADAPTER_PERFDATA.Temperature, in 0.1 °C

#[repr(C)]
#[derive(Clone, Copy)]
struct KmtLuid {
    low: u32,
    high: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct KmtAdapterInfo {
    adapter: u32,
    luid: KmtLuid,
    num_sources: u32,
    precise_present_regions: i32,
}

#[repr(C)]
struct KmtEnumerateAdapters2 {
    adapter_count: u32,
    adapters: *mut KmtAdapterInfo,
}

#[repr(C)]
struct KmtQueryAdapterInfo {
    adapter: u32,
    kind: i32,
    driver_data: *mut c_void,
    driver_data_size: u32,
}

// gdi32 forwards these kernel thunks; they are not in the windows crate.
#[link(name = "gdi32")]
extern "system" {
    fn D3DKMTEnumAdapters2(adapters: *mut KmtEnumerateAdapters2) -> i32;
    fn D3DKMTQueryAdapterInfo(info: *mut KmtQueryAdapterInfo) -> i32;
}

struct PdhQuery {
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
}

impl Drop for PdhQuery {
    fn drop(&mut self) {
        unsafe { PdhCloseQuery(self.query) };
    }
}

pub struct WinGpu {
    pub name: Option<String>,
    luid: Option<(u32, i32)>, // (low, high)
    adapter: Option<u32>,
    counters: Option<PdhQuery>,
}

impl Default for WinGpu {
    fn default() -> Self {
        Self::new()
    }
}

impl WinGpu {
    pub fn new() -> Self {
        let mut gpu = Self {
            name: None,
            luid: None,
            adapter: None,
            counters: None,
        };
        // ctypes/COM-style best effort: a failure here must never break the app.
        let _ = gpu.pick_adapter();
        let _ = gpu.open_counters();
        gpu
    }

    fn pick_adapter(&mut self) -> Option<()> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
            let mut best: Option<(String, usize, u32, i32)> = None;
            for index in 0.. {
                let Ok(adapter) = factory.EnumAdapters1(index) else {
                    break;
                };
                let Ok(desc) = adapter.GetDesc1() else {
                    continue;
                };
                let vram = desc.DedicatedVideoMemory;
                if vram == 0 {
                    continue;
                }
                if best
                    .as_ref()
                    .is_none_or(|(_, best_vram, _, _)| vram > *best_vram)
                {
                    let end = desc
                        .Description
                        .iter()
                        .position(|c| *c == 0)
                        .unwrap_or(desc.Description.len());
                    let name = String::from_utf16_lossy(&desc.Description[..end]);
                    best = Some((
                        name,
                        vram,
                        desc.AdapterLuid.LowPart,
                        desc.AdapterLuid.HighPart,
                    ));
                }
            }
            drop(factory);
            let (name, _, low, high) = best?;
            self.name = Some(name);
            self.luid = Some((low, high));
            self.adapter = find_kmt_handle(low, high);
            Some(())
        }
    }

    fn open_counters(&mut self) -> Option<()> {
        unsafe {
            let mut query = PDH_HQUERY::default();
            if PdhOpenQueryW(None, 0, &mut query) != 0 {
                return None;
            }
            let mut counter = PDH_HCOUNTER::default();
            let path = windows::core::w!(r"\GPU Engine(*)\Utilization Percentage");
            if PdhAddEnglishCounterW(query, path, 0, &mut counter) != 0 {
                let _ = PdhCloseQuery(query);
                return None;
            }
            // Prime the counter: the first sample has no delta yet.
            let _ = PdhCollectQueryData(query);
            self.counters = Some(PdhQuery { query, counter });
            Some(())
        }
    }

    /// Hottest GPU reading from the graphics kernel, in °C.
    pub fn temperature(&self) -> Option<f64> {
        let adapter = self.adapter?;
        unsafe {
            let mut buffer = [0u8; PERFDATA_SIZE as usize];
            let mut info = KmtQueryAdapterInfo {
                adapter,
                kind: PERFDATA_TYPE,
                driver_data: buffer.as_mut_ptr().cast(),
                driver_data_size: PERFDATA_SIZE,
            };
            if D3DKMTQueryAdapterInfo(&mut info) != 0 {
                return None;
            }
            let deci_c = u32::from_le_bytes([
                buffer[TEMP_OFFSET],
                buffer[TEMP_OFFSET + 1],
                buffer[TEMP_OFFSET + 2],
                buffer[TEMP_OFFSET + 3],
            ]);
            let temp = deci_c as f64 / 10.0;
            (0.0 < temp && temp < 150.0).then_some(temp)
        }
    }

    /// Busiest GPU engine utilisation percentage for this adapter.
    pub fn load(&mut self) -> Option<f64> {
        let (low, high) = self.luid?;
        let query = self.counters.as_mut()?;

        unsafe {
            if PdhCollectQueryData(query.query) != 0 {
                return None;
            }

            let mut size = 0u32;
            let mut count = 0u32;
            let first = PdhGetFormattedCounterArrayW(
                query.counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                None,
            );
            if first != PDH_MORE_DATA && first != 0 {
                return None;
            }
            if size == 0 {
                return None;
            }

            // PDH_FMT_COUNTERVALUE_ITEM_W contains pointers and doubles, so the
            // buffer must be 8-byte aligned; Vec<u64> gives us that for free.
            let mut buffer: Vec<u64> = vec![0; (size as usize).div_ceil(8)];
            let status = PdhGetFormattedCounterArrayW(
                query.counter,
                PDH_FMT_DOUBLE,
                &mut size,
                &mut count,
                Some(buffer.as_mut_ptr().cast()),
            );
            if status != 0 {
                return None;
            }

            let items = std::slice::from_raw_parts(
                buffer.as_ptr() as *const PDH_FMT_COUNTERVALUE_ITEM_W,
                count as usize,
            );
            let tag = format!("luid_0x{:08x}_0x{low:08x}", high as u32);
            // Sum each engine type across processes, then report the busiest -
            // summing every engine double-counts (copy/video run alongside 3D).
            let mut per_engine: std::collections::HashMap<String, f64> =
                std::collections::HashMap::new();
            for item in items {
                let name = item
                    .szName
                    .to_string()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if !name.contains(&tag) {
                    continue;
                }
                let Some(engine) = engine_type(&name) else {
                    continue;
                };
                let value = item.FmtValue.Anonymous.doubleValue;
                *per_engine.entry(engine.to_string()).or_insert(0.0) += value;
            }
            if per_engine.is_empty() {
                return None;
            }
            Some(
                per_engine
                    .values()
                    .copied()
                    .fold(0.0f64, f64::max)
                    .clamp(0.0, 100.0),
            )
        }
    }
}

fn engine_type(name: &str) -> Option<&str> {
    let rest = name.split("engtype_").nth(1)?;
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric())
        .unwrap_or(rest.len());
    (end > 0).then_some(&rest[..end])
}

fn find_kmt_handle(low: u32, high: i32) -> Option<u32> {
    unsafe {
        let mut enumerate = KmtEnumerateAdapters2 {
            adapter_count: 0,
            adapters: std::ptr::null_mut(),
        };
        if D3DKMTEnumAdapters2(&mut enumerate) != 0 || enumerate.adapter_count == 0 {
            return None;
        }
        let mut adapters = vec![
            KmtAdapterInfo {
                adapter: 0,
                luid: KmtLuid { low: 0, high: 0 },
                num_sources: 0,
                precise_present_regions: 0,
            };
            enumerate.adapter_count as usize
        ];
        enumerate.adapters = adapters.as_mut_ptr();
        if D3DKMTEnumAdapters2(&mut enumerate) != 0 {
            return None;
        }
        adapters
            .iter()
            .find(|a| a.luid.low == low && a.luid.high == high)
            .map(|a| a.adapter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_tags_are_parsed() {
        let name = "pid_123_luid_0x00000000_0x0000abcd_phys_0_eng_0_engtype_3D";
        assert_eq!(engine_type(name), Some("3D"));
        assert_eq!(engine_type("no-engine-here"), None);
    }

    #[test]
    fn perfdata_structs_match_the_abi() {
        assert_eq!(std::mem::size_of::<KmtLuid>(), 8);
        assert_eq!(std::mem::size_of::<KmtAdapterInfo>(), 20);
        assert_eq!(std::mem::size_of::<KmtQueryAdapterInfo>(), 24);
        // The graphics kernel writes a 0.1 °C ULONG at offset 56.
        assert_eq!(TEMP_OFFSET + 4, 60);
    }
}
