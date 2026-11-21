//! The boot services and runtime services tables: every function, the
//! address this firmware put in the table for it, and what it is for.

use crate::handles;
use crate::systab;
use crate::tui::{self, BrowseState, Doc, Item, Outcome, Screen, st};
use crate::util;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use uefi::boot::{self, MemoryType, SearchType};
use uefi::mem::memory_map::MemoryMap;
use uefi::runtime::{self, VariableAttributes};
use uefi::table;

struct Service {
    name: &'static str,
    /// The C prototype's arguments, abbreviated.
    args: &'static str,
    about: &'static str,
}

const fn s(name: &'static str, args: &'static str, about: &'static str) -> Service {
    Service { name, args, about }
}

/// Group titles, and the index of the first service in each.
const BOOT_GROUPS: &[(usize, &str)] = &[
    (0, "Task priority"),
    (2, "Memory"),
    (7, "Events and timers"),
    (13, "Protocol handlers"),
    (22, "Images"),
    (27, "Miscellaneous"),
    (30, "Driver support"),
    (32, "Opening protocols"),
    (35, "Library"),
    (40, "CRC and memory"),
    (43, "Events, again"),
];

const BOOT: &[Service] = &[
    s("RaiseTPL", "NewTpl", "Raises the task priority level, which holds back event callbacks at or below it. TPL is UEFI's substitute for locks: code at a high TPL can't be interrupted by callbacks at a lower one. Applications run at the lowest, TPL_APPLICATION."),
    s("RestoreTPL", "OldTpl", "Puts the task priority level back, and runs the event callbacks that were held back meanwhile."),
    s("AllocatePages", "Type, MemoryType, Pages, *Memory", "Allocates whole 4 KiB pages, anywhere, below an address, or at a fixed address. A loader uses the fixed form to put a kernel where it was linked to run. The memory type given here is what the range will show as in the memory map."),
    s("FreePages", "Memory, Pages", "Returns pages to the firmware."),
    s("GetMemoryMap", "*Size, *Map, *MapKey, *DescSize, *DescVersion", "Returns the list of every memory range and its type, and a key that identifies this version of the map. ExitBootServices() takes that key."),
    s("AllocatePool", "PoolType, Size, **Buffer", "Allocates a block of any size, 8-byte aligned: malloc(). This program's own heap comes from here."),
    s("FreePool", "*Buffer", "Frees a block from AllocatePool(): free()."),
    s("CreateEvent", "Type, NotifyTpl, NotifyFunction, *Context, *Event", "Creates an event: something that can be waited for, signalled, or tied to a timer, with an optional callback. A callback can also be registered for the moment ExitBootServices() is called, which is how drivers quiesce their hardware."),
    s("SetTimer", "Event, Type, TriggerTime", "Makes an event fire once or periodically. The time is in units of 100 nanoseconds."),
    s("WaitForEvent", "NumberOfEvents, *Event, *Index", "Blocks until one of several events is signalled. This program is sitting in this call right now, waiting for the keyboard's event."),
    s("SignalEvent", "Event", "Signals an event, queueing its callback."),
    s("CloseEvent", "Event", "Destroys an event."),
    s("CheckEvent", "Event", "Asks whether an event is signalled, without blocking."),
    s("InstallProtocolInterface", "*Handle, *Protocol, InterfaceType, *Interface", "Adds a protocol to a handle, creating the handle if none is given. This is how every device and service in the handle database comes to exist."),
    s("ReinstallProtocolInterface", "Handle, *Protocol, *OldInterface, *NewInterface", "Replaces a protocol's interface on a handle and notifies everyone who registered an interest. Used when media changes, for instance."),
    s("UninstallProtocolInterface", "Handle, *Protocol, *Interface", "Removes a protocol from a handle. The handle disappears with its last protocol."),
    s("HandleProtocol", "Handle, *Protocol, **Interface", "The original, simple way to get a protocol's interface from a handle. OpenProtocol() replaced it: it also records who is using the interface."),
    s("(reserved)", "", "An unused slot, kept so that the table's layout matches the original EFI 1.0."),
    s("RegisterProtocolNotify", "*Protocol, Event, **Registration", "Signals an event whenever a protocol is installed anywhere. A driver waiting for a device that appears late uses it."),
    s("LocateHandle", "SearchType, *Protocol, *SearchKey, *BufferSize, *Buffer", "Fills a caller's buffer with the handles that carry a protocol. LocateHandleBuffer() is the easier form."),
    s("LocateDevicePath", "*Protocol, **DevicePath, *Device", "Finds the handle furthest along a device path that carries a protocol, and returns the rest of the path. Given a path to a file, it finds the file system the file is on."),
    s("InstallConfigurationTable", "*Guid, *Table", "Adds, replaces or removes an entry of the system table's configuration table. Drivers publish ACPI and SMBIOS this way; Linux's loader adds tables of its own for the kernel."),
    s("LoadImage", "BootPolicy, ParentImageHandle, *DevicePath, *SourceBuffer, SourceSize, *ImageHandle", "Loads a PE/COFF image from a device path or from memory: checks it against Secure Boot, relocates it, and creates a handle for it with a Loaded Image protocol. It doesn't run it."),
    s("StartImage", "ImageHandle, *ExitDataSize, **ExitData", "Calls a loaded image's entry point and returns when the image exits. This is the call the firmware made to start this program, and the one this program makes to start your OS loader."),
    s("Exit", "ImageHandle, ExitStatus, ExitDataSize, *ExitData", "Ends the calling image from anywhere in its code and returns control to whoever called StartImage()."),
    s("UnloadImage", "ImageHandle", "Removes an image from memory. For a driver, its unload function is called first."),
    s("ExitBootServices", "ImageHandle, MapKey", "The point of no return. The firmware signals every driver to stop, the boot services table becomes invalid, and the caller owns the machine. It fails if the map key is stale, in which case the loader must get the memory map again and retry."),
    s("GetNextMonotonicCount", "*Count", "Returns a 64-bit number that increases with every call and never repeats, even across resets (the high half is kept in a variable)."),
    s("Stall", "Microseconds", "Busy-waits. There is no sleep that yields to anything else: there is nothing else."),
    s("SetWatchdogTimer", "Timeout, WatchdogCode, DataSize, *WatchdogData", "Arms a timer that resets the machine if it expires, or disarms it with a timeout of zero. The boot manager sets five minutes before starting a boot option; ExitBootServices() disarms it."),
    s("ConnectController", "ControllerHandle, *DriverImageHandle, *RemainingDevicePath, Recursive", "Finds drivers for a controller: asks every Driver Binding protocol whether it supports the handle, and starts the ones that do. Recursively, it brings up everything behind a bus. Firmware that boots fast connects only what the boot option needs."),
    s("DisconnectController", "ControllerHandle, DriverImageHandle, ChildHandle", "Stops the drivers managing a controller."),
    s("OpenProtocol", "Handle, *Protocol, **Interface, AgentHandle, ControllerHandle, Attributes", "Gets a protocol's interface from a handle and records who opened it and how. A driver opens BY_DRIVER, which keeps a second driver off the same device; an application just looks (GET_PROTOCOL), as this program does."),
    s("CloseProtocol", "Handle, *Protocol, AgentHandle, ControllerHandle", "Undoes OpenProtocol()."),
    s("OpenProtocolInformation", "Handle, *Protocol, **EntryBuffer, *EntryCount", "Lists who has a protocol open on a handle. It shows which driver is managing which device."),
    s("ProtocolsPerHandle", "Handle, ***ProtocolBuffer, *ProtocolBufferCount", "Lists the GUIDs of the protocols on a handle. The Handles screen calls it for each handle."),
    s("LocateHandleBuffer", "SearchType, *Protocol, *SearchKey, *NoHandles, **Buffer", "Returns an allocated array of all handles, or of those that carry a protocol. The first step of finding any device."),
    s("LocateProtocol", "*Protocol, *Registration, **Interface", "Returns the first interface found for a protocol, for services there is only one of."),
    s("InstallMultipleProtocolInterfaces", "*Handle, ...", "Installs several protocols on a handle at once, or none if any fails."),
    s("UninstallMultipleProtocolInterfaces", "Handle, ...", "Removes several protocols from a handle at once."),
    s("CalculateCrc32", "*Data, DataSize, *Crc32", "Computes the CRC32 used in table headers and in the GPT partition table."),
    s("CopyMem", "*Destination, *Source, Length", "memmove(). In the table so that the smallest image needs no library."),
    s("SetMem", "*Buffer, Size, Value", "memset()."),
    s("CreateEventEx", "Type, NotifyTpl, NotifyFunction, *Context, *EventGroup, *Event", "CreateEvent() with an event group: a GUID that ties events together so that signalling one signals all. \"Ready to boot\" and \"exit boot services\" are groups."),
];

const RUNTIME_GROUPS: &[(usize, &str)] = &[(0, "Time"), (4, "Virtual memory"), (6, "Variables"), (9, "Miscellaneous"), (11, "Capsules"), (13, "Variables")];

const RUNTIME: &[Service] = &[
    s("GetTime", "*Time, *Capabilities", "Reads the real-time clock: date, time, time zone and daylight saving flag."),
    s("SetTime", "*Time", "Sets the real-time clock."),
    s("GetWakeupTime", "*Enabled, *Pending, *Time", "Reads the alarm that can wake the machine."),
    s("SetWakeupTime", "Enable, *Time", "Sets or clears the wake-up alarm."),
    s("SetVirtualAddressMap", "MapSize, DescSize, DescVersion, *VirtualMap", "Called once by the OS, after ExitBootServices(), to tell the firmware the virtual addresses it has given the runtime memory ranges. The firmware fixes up its own pointers, and from then on runtime services are called through the new mapping."),
    s("ConvertPointer", "DebugDisposition, **Address", "Used by runtime drivers during SetVirtualAddressMap() to convert one of their pointers to its new virtual address."),
    s("GetVariable", "*Name, *VendorGuid, *Attributes, *DataSize, *Data", "Reads a variable's value and attributes by name and vendor GUID."),
    s("GetNextVariableName", "*NameSize, *Name, *VendorGuid", "Enumerates variables: given one name, returns the next. The Variables screen walks the whole store with it."),
    s("SetVariable", "*Name, *VendorGuid, Attributes, DataSize, *Data", "Creates, changes or (with a size of zero) deletes a variable. An OS installer writes its Boot#### option and BootOrder with this."),
    s("GetNextHighMonotonicCount", "*HighCount", "Returns and increments the high 32 bits of the monotonic counter, which are kept in a non-volatile variable."),
    s("ResetSystem", "ResetType, ResetStatus, DataSize, *ResetData", "Restarts the machine (cold or warm) or powers it off. It doesn't return."),
    s("UpdateCapsule", "**CapsuleHeaderArray, CapsuleCount, ScatterGatherList", "Passes capsules to the firmware: blobs identified by GUID, most often firmware updates to be applied across a reset."),
    s("QueryCapsuleCapabilities", "**CapsuleHeaderArray, CapsuleCount, *MaximumCapsuleSize, *ResetType", "Asks whether a capsule would be accepted, how large it may be, and what kind of reset it needs."),
    s("QueryVariableInfo", "Attributes, *MaximumStorage, *RemainingStorage, *MaximumVariableSize", "Reports how much space the variable store has and how much is left."),
];

/// The address in each slot of the boot services table, in table order.
fn boot_addrs() -> Vec<usize> {
    // SAFETY: the table is valid while boot services are active.
    let bt = unsafe { table::system_table_raw().unwrap().as_ref().boot_services.as_ref().unwrap() };
    alloc::vec![
        bt.raise_tpl as usize,
        bt.restore_tpl as usize,
        bt.allocate_pages as usize,
        bt.free_pages as usize,
        bt.get_memory_map as usize,
        bt.allocate_pool as usize,
        bt.free_pool as usize,
        bt.create_event as usize,
        bt.set_timer as usize,
        bt.wait_for_event as usize,
        bt.signal_event as usize,
        bt.close_event as usize,
        bt.check_event as usize,
        bt.install_protocol_interface as usize,
        bt.reinstall_protocol_interface as usize,
        bt.uninstall_protocol_interface as usize,
        bt.handle_protocol as usize,
        bt.reserved as usize,
        bt.register_protocol_notify as usize,
        bt.locate_handle as usize,
        bt.locate_device_path as usize,
        bt.install_configuration_table as usize,
        bt.load_image as usize,
        bt.start_image as usize,
        bt.exit as usize,
        bt.unload_image as usize,
        bt.exit_boot_services as usize,
        bt.get_next_monotonic_count as usize,
        bt.stall as usize,
        bt.set_watchdog_timer as usize,
        bt.connect_controller as usize,
        bt.disconnect_controller as usize,
        bt.open_protocol as usize,
        bt.close_protocol as usize,
        bt.open_protocol_information as usize,
        bt.protocols_per_handle as usize,
        bt.locate_handle_buffer as usize,
        bt.locate_protocol as usize,
        bt.install_multiple_protocol_interfaces as usize,
        bt.uninstall_multiple_protocol_interfaces as usize,
        bt.calculate_crc32 as usize,
        bt.copy_mem as usize,
        bt.set_mem as usize,
        bt.create_event_ex as usize,
    ]
}

fn runtime_addrs() -> Vec<usize> {
    // SAFETY: the table is valid for the life of the system.
    let rt = unsafe { table::system_table_raw().unwrap().as_ref().runtime_services.as_ref().unwrap() };
    alloc::vec![
        rt.get_time as usize,
        rt.set_time as usize,
        rt.get_wakeup_time as usize,
        rt.set_wakeup_time as usize,
        rt.set_virtual_address_map as usize,
        rt.convert_pointer as usize,
        rt.get_variable as usize,
        rt.get_next_variable_name as usize,
        rt.set_variable as usize,
        rt.get_next_high_monotonic_count as usize,
        rt.reset_system as usize,
        rt.update_capsule as usize,
        rt.query_capsule_capabilities as usize,
        rt.query_variable_info as usize,
    ]
}

/// What calling a service returns here and now, for the ones that are
/// harmless to call.
fn live(doc: &mut Doc, name: &str) {
    match name {
        "GetMemoryMap" => {
            if let Ok(map) = boot::memory_map(MemoryType::LOADER_DATA) {
                doc.kv("Entries now", format!("{}", map.len()));
                doc.kv("Entry size", format!("{} bytes", map.meta().desc_size));
            }
        }
        "LocateHandleBuffer" => {
            if let Ok(all) = boot::locate_handle_buffer(SearchType::AllHandles) {
                doc.kv("All handles", format!("{}", all.len()));
            }
        }
        "CalculateCrc32" => {
            if let Ok(crc) = boot::calculate_crc32(b"weefy") {
                doc.kv("Of \"weefy\"", format!("{crc:#010x}"));
            }
        }
        "GetNextMonotonicCount" => {
            // SAFETY: a call with a valid pointer for its one result.
            let bt = unsafe { table::system_table_raw().unwrap().as_ref().boot_services.as_ref().unwrap() };
            let mut count = 0u64;
            if unsafe { (bt.get_next_monotonic_count)(&mut count) }.is_success() {
                doc.kv("Returned", format!("{count:#x}  (press r to call it again)"));
            }
        }
        "GetTime" => match runtime::get_time_and_caps() {
            Ok((t, caps)) => {
                doc.kv(
                    "Now",
                    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.year(), t.month(), t.day(), t.hour(), t.minute(), t.second()),
                );
                doc.kv(
                    "Time zone",
                    match t.time_zone() {
                        Some(tz) => format!("UTC{:+} minutes", tz),
                        None => String::from("unspecified (local time)"),
                    },
                );
                doc.kv("Resolution", format!("{} Hz", caps.resolution));
                doc.dim("Press r to read the clock again.");
            }
            Err(e) => doc.kv("Returned", format!("{:?}", e.status())),
        },
        "QueryVariableInfo" => {
            let attrs = VariableAttributes::NON_VOLATILE | VariableAttributes::BOOTSERVICE_ACCESS | VariableAttributes::RUNTIME_ACCESS;
            match runtime::query_variable_info(attrs) {
                Ok(info) => {
                    doc.dim("For non-volatile variables visible at runtime:");
                    doc.kv("Store size", util::size(info.maximum_variable_storage_size));
                    doc.kv("Remaining", util::size(info.remaining_variable_storage_size));
                    doc.kv("Largest variable", util::size(info.maximum_variable_size));
                }
                Err(e) => doc.kv("Returned", format!("{:?}", e.status())),
            }
        }
        "GetNextVariableName" => {
            doc.kv("Variables", format!("{}", runtime::variable_keys().filter(|k| k.is_ok()).count()));
        }
        _ => {}
    }
}

fn run(scr: &mut Screen, runtime_table: bool) {
    let (services, groups, addrs, crumbs) = if runtime_table {
        (RUNTIME, RUNTIME_GROUPS, runtime_addrs(), "Runtime services")
    } else {
        (BOOT, BOOT_GROUPS, boot_addrs(), "Boot services")
    };
    // Rows: the table header, then group headers and services. `map` gives
    // the service index of each row.
    let mut items = alloc::vec![Item::new("The table")];
    let mut map: Vec<Option<usize>> = alloc::vec![None];
    for (i, svc) in services.iter().enumerate() {
        if let Some(g) = groups.iter().find(|g| g.0 == i) {
            items.push(Item::header(g.1));
            map.push(None);
        }
        let item = Item::new(svc.name);
        items.push(if svc.args.is_empty() { item.styled(st::DIM) } else { item });
        map.push(Some(i));
    }
    let images = handles::images();
    let rt_mask = systab::rt_properties();
    let mut state = BrowseState::default();
    loop {
        let out = tui::browse(scr, &mut state, crumbs, crumbs, &items, &[("r", "Refresh")], &mut |row, width| {
            let mut doc = Doc::new(width);
            let Some(i) = map[row] else {
                // SAFETY: the tables are valid while boot services run.
                let st_raw = unsafe { table::system_table_raw().unwrap().as_ref() };
                if runtime_table {
                    doc.p("The services that stay available to the OS after ExitBootServices(). The table and the code it points to sit in memory of the runtime services types, which the OS must keep and map.");
                    doc.blank();
                    doc.kv("Address", format!("{:#x}", st_raw.runtime_services as usize));
                    // SAFETY: as above.
                    systab::header(&mut doc, unsafe { &(*st_raw.runtime_services).header }, "the UEFI version");
                    doc.blank();
                    match rt_mask {
                        Some(mask) => doc.p(&format!(
                            "This firmware publishes an RT properties table: {} of the 14 services work once the OS is running. Each service's page says which.",
                            mask.count_ones()
                        )),
                        None => doc.dim("This firmware publishes no RT properties table, which means all of these are meant to work once the OS is running."),
                    }
                } else {
                    doc.p("The services the firmware offers while it is in charge. An image reaches them through the pointer in the system table. All of it becomes invalid at ExitBootServices().");
                    doc.blank();
                    doc.kv("Address", format!("{:#x}", st_raw.boot_services as usize));
                    // SAFETY: as above.
                    systab::header(&mut doc, unsafe { &(*st_raw.boot_services).header }, "the UEFI version");
                }
                doc.kv("Functions", format!("{}", services.len()));
                return doc;
            };
            let svc = &services[i];
            doc.p(svc.about);
            if !svc.args.is_empty() {
                doc.blank();
                doc.styled(st::KEY, &format!("{}({})", svc.name, svc.args));
            }
            doc.h("In this firmware");
            doc.kv("Slot", format!("{} of {}", i + 1, services.len()));
            doc.kv("Points to", format!("{:#x}", addrs[i]));
            if addrs[i] != 0 {
                match images.iter().find(|im| addrs[i] >= im.base && addrs[i] < im.base + im.size) {
                    Some(im) => doc.kv("Inside image", format!("{} (loaded at {:#x})", im.name, im.base)),
                    None => doc.kv("Inside image", "none of the loaded images"),
                }
            }
            if runtime_table {
                if let Some(mask) = rt_mask {
                    let on = mask & (1 << i) != 0;
                    doc.kv_styled(
                        "After the OS starts",
                        if on { st::GOOD } else { st::BAD },
                        if on { "supported" } else { "not supported (RT properties table)" },
                    );
                }
            }
            let before = doc.lines.len();
            doc.h("Calling it now");
            let mark = doc.lines.len();
            live(&mut doc, svc.name);
            if doc.lines.len() == mark {
                doc.lines.truncate(before);
            }
            doc
        });
        match out {
            Outcome::Back => return,
            Outcome::Open(_) => state.zoom = true,
            Outcome::Char(_) => {}
        }
    }
}

pub fn run_boot(scr: &mut Screen) {
    run(scr, false);
}

pub fn run_runtime(scr: &mut Screen) {
    run(scr, true);
}
