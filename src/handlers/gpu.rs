use serde::Serialize;
use ash::vk;
use std::ffi::CStr;
use bollard::models::{DeviceRequest, DeviceMapping};

#[derive(Serialize, Clone, Debug)]
pub struct GpuInfo {
    pub id: String,
    pub name: String,
    pub vendor: String,
}

#[derive(Default)]
pub struct GpuDockerConfig {
    pub device_requests: Vec<DeviceRequest>,
    pub devices: Vec<DeviceMapping>,
    pub security_opts: Vec<String>,
    pub group_adds: Vec<String>,
    pub env: Vec<String>,
}

pub fn get_system_gpus() -> Vec<GpuInfo> {
    match get_vulkan_gpus() {
        Ok(gpus) if !gpus.is_empty() => gpus,
        _ => get_fallback_gpus(),
    }
}

pub fn resolve_gpu_config(gpu_id: &str) -> GpuDockerConfig {
    let mut config = GpuDockerConfig::default();

    if gpu_id == "nvidia_all" {
        config.device_requests.push(DeviceRequest {
            driver: Some("cdi".to_string()),
            device_ids: Some(vec!["nvidia.com/gpu=all".to_string()]),
            ..Default::default()
        });

        config.security_opts.push("label=disable".to_string());
        config.env.push("NVIDIA_VISIBLE_DEVICES=all".to_string());
        config.env.push("NVIDIA_DRIVER_CAPABILITIES=all".to_string());

        return config;
    }

    if let Some(uuid) = gpu_id.strip_prefix("nvidia_uuid_") {
        // Pass specific UUID to device_ids if possible, or use NVIDIA_VISIBLE_DEVICES env
        // Docker's DeviceRequest supports device_ids
        let cdi_name = format!("nvidia.com/gpu=GPU-{}", uuid);
        config.device_requests.push(DeviceRequest {
            driver: Some("cdi".to_string()),
            device_ids: Some(vec![cdi_name]),
            ..Default::default()
        });

        config.security_opts.push("label=disable".to_string());
        config.env.push("NVIDIA_VISIBLE_DEVICES=all".to_string());
        config.env.push("NVIDIA_DRIVER_CAPABILITIES=all".to_string());

        return config;
    }

    if gpu_id == "render_device" {
         // Fallback legacy behavior
         add_drm_devices(&mut config, None);
         return config;
    }

    if let Some(pci) = gpu_id.strip_prefix("drm_pci_") {
        add_drm_devices(&mut config, Some(pci));
        return config;
    }

    config
}

fn add_drm_devices(config: &mut GpuDockerConfig, pci_match: Option<&str>) {
    // If pci_match is None, add all render nodes (legacy behavior)
    // If pci_match is Some, find the specific render node
    
    if let Some(pci) = pci_match {
        if let Some(render_path) = find_render_node_by_pci(pci) {
            config.devices.push(DeviceMapping {
                path_on_host: Some(render_path.clone()),
                path_in_container: Some("/dev/dri/renderD128".to_string()), // Map to default inside
                cgroup_permissions: Some("rwm".to_string()),
            });
            // Try to find associated card node too? 
            // For now just render node is usually enough for compute/encode.
            // But let's try to be nice.
        }
    } else {
        // Legacy: Add renderD128 and card0 if they exist
        if std::path::Path::new("/dev/dri/renderD128").exists() {
            config.devices.push(DeviceMapping {
                path_on_host: Some("/dev/dri/renderD128".to_string()),
                path_in_container: Some("/dev/dri/renderD128".to_string()),
                cgroup_permissions: Some("rwm".to_string()),
            });
        }
        if std::path::Path::new("/dev/dri/card0").exists() {
            config.devices.push(DeviceMapping {
                path_on_host: Some("/dev/dri/card0".to_string()),
                path_in_container: Some("/dev/dri/card0".to_string()),
                cgroup_permissions: Some("rwm".to_string()),
            });
        }
    }
}

fn find_render_node_by_pci(target_pci: &str) -> Option<String> {
    let dri_dir = std::fs::read_dir("/dev/dri").ok()?;
    for entry in dri_dir {
        let entry = entry.ok()?;
        let path = entry.path();
        if let Some(fname) = path.file_name().and_then(|n| n.to_str()) {
            if fname.starts_with("renderD") {
                // Check sysfs
                let sys_path = format!("/sys/class/drm/{}/device", fname);
                // The 'device' symlink points to the PCI device directory
                if let Ok(target) = std::fs::read_link(&sys_path) {
                    // target is something like "../../../0000:00:02.0"
                    // We want to extract the last component "0000:00:02.0"
                    if let Some(pci_comp) = target.file_name().and_then(|n| n.to_str()) {
                        if pci_comp == target_pci {
                            return Some(path.to_string_lossy().to_string());
                        }
                    }
                }
                
                // Alternative: check uevent file
                let uevent_path = format!("/sys/class/drm/{}/device/uevent", fname);
                if let Ok(content) = std::fs::read_to_string(uevent_path) {
                    for line in content.lines() {
                        if line.starts_with("PCI_SLOT_NAME=") {
                            let slot = line.strip_prefix("PCI_SLOT_NAME=").unwrap();
                            if slot == target_pci {
                                return Some(path.to_string_lossy().to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    None
}

pub fn get_gpu_usage() -> Option<f64> {
    // 1. Try NVIDIA (if exists)
    if std::path::Path::new("/dev/nvidia0").exists() {
        if let Ok(output) = std::process::Command::new("nvidia-smi")
            .args(&["--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"])
            .output() {
            if output.status.success() {
                let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if let Ok(usage) = s.parse::<f64>() {
                    return Some(usage);
                }
            }
        }
    }

    // 2. Try Intel/AMD (via sysfs)
    let dri_nodes = ["renderD128", "renderD129", "card0", "card1"];
    for node in dri_nodes {
        // AMD gpu_busy_percent
        let path = format!("/sys/class/drm/{}/device/gpu_busy_percent", node);
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(usage) = content.trim().parse::<f64>() {
                return Some(usage);
            }
        }
        
        // Intel usage_stats (newer kernels/drivers)
        // This is harder to parse without complex logic, skipping for now
    }

    None
}

fn get_fallback_gpus() -> Vec<GpuInfo> {
    let mut gpus = Vec::new();
    if std::path::Path::new("/dev/nvidia0").exists() {
        gpus.push(GpuInfo { 
            id: "nvidia_all".to_string(), 
            name: "NVIDIA GPU (All)".to_string(),
            vendor: "NVIDIA".to_string()
        });
    }
    if std::path::Path::new("/dev/dri/renderD128").exists() {
        gpus.push(GpuInfo { 
            id: "render_device".to_string(), 
            name: "VAAPI/QuickSync (renderD128)".to_string(),
            vendor: "Intel/AMD".to_string()
        });
    }
    gpus
}

fn get_vulkan_gpus() -> Result<Vec<GpuInfo>, Box<dyn std::error::Error>> {
    let entry = unsafe { ash::Entry::load()? };
    let app_info = vk::ApplicationInfo::builder()
        .api_version(vk::make_api_version(0, 1, 1, 0)); // Vulkan 1.1
        
    let create_info = vk::InstanceCreateInfo::builder()
        .application_info(&app_info);
        
    let instance = unsafe { entry.create_instance(&create_info, None)? };
    
    let pdevices = unsafe { instance.enumerate_physical_devices()? };
    let mut gpus = Vec::new();
    
    for pdevice in pdevices {
        let props = unsafe { instance.get_physical_device_properties(pdevice) };
        
        let name = unsafe { 
            CStr::from_ptr(props.device_name.as_ptr())
                .to_string_lossy()
                .into_owned() 
        };

        if name.to_lowercase().contains("llvmpipe") {
            continue;
        }
        
        // Vendor ID check
        let vendor = match props.vendor_id {
            0x10DE => "NVIDIA",
            0x8086 => "Intel",
            0x1002 => "AMD",
            _ => "Unknown",
        }.to_string();

        // Try to get UUID and PCI info using get_physical_device_properties2
        // We need to use the instance function for this.
        // If Vulkan 1.1 is supported, this function is core.
        
        // Check for PCI Bus Info extension support? 
        // Or just try to chain it. If extension not enabled, it might be ignored or fail?
        // Actually, to use pNext structs, we usually need to enable the extension in Instance creation if it's an instance extension?
        // VK_EXT_pci_bus_info is a device extension usually? No, it's retrieved from physical device.
        // It requires VK_KHR_get_physical_device_properties2 (core in 1.1) and VK_EXT_pci_bus_info.
        
        // Let's check if VK_EXT_pci_bus_info is available.
        // For simplicity, we can assume if it's not available, we skip PCI ID.
        
        // We will try to construct a unique ID.
        let id;
        
        if vendor == "NVIDIA" {
             // For NVIDIA, we prefer UUID.
             // We can try to get it from IDProperties (Vulkan 1.1)
             // Let's attempt to use GetPhysicalDeviceProperties2
             // We need to load the function or use instance wrapper if ash provides it on Instance.
             // ash::Instance has get_physical_device_properties2.
             
             let mut id_props = vk::PhysicalDeviceIDProperties::default();
             let mut props2 = vk::PhysicalDeviceProperties2::builder()
                 .push_next(&mut id_props);
                 
             unsafe { instance.get_physical_device_properties2(pdevice, &mut props2) };
             
             // uuid is [u8; 16]
             let uuid_bytes = id_props.device_uuid;
             let uuid_str = uuid::Uuid::from_bytes(uuid_bytes).to_string();
             id = format!("nvidia_uuid_{}", uuid_str);
        } else {
             // For Intel/AMD, we want PCI bus info to map to /dev/dri
             // Try to use VK_EXT_pci_bus_info
             // We need to enable the extension? No, just querying props usually doesn't require enabling extension if it's promoted or available?
             // Actually, to use the struct, the extension must be supported by the instance/device.
             // Let's try to query it.
             
             let mut pci_props = vk::PhysicalDevicePCIBusInfoPropertiesEXT::default();
             let mut props2 = vk::PhysicalDeviceProperties2::builder()
                 .push_next(&mut pci_props);
                 
             unsafe { instance.get_physical_device_properties2(pdevice, &mut props2) };
             
             if pci_props.pci_domain > 0 || pci_props.pci_bus > 0 || pci_props.pci_device > 0 {
                 let pci_str = format!("{:04x}:{:02x}:{:02x}.{:x}", 
                     pci_props.pci_domain, pci_props.pci_bus, pci_props.pci_device, pci_props.pci_function);
                 id = format!("drm_pci_{}", pci_str);
             } else {
                 // Fallback if PCI info is empty/zero (extension not supported)
                 // Use index-based or just generic
                 id = format!("generic_{}_{}", vendor, props.device_id);
             }
        }
        
        gpus.push(GpuInfo {
            id,
            name,
            vendor,
        });
    }
    
    Ok(gpus)
}
