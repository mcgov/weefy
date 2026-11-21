//! The guided tour: short lessons, each tied to what this machine shows.

use crate::bootopt;
use crate::tui::{self, BrowseState, Doc, Item, Outcome, Screen, st};
use crate::util;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use uefi::boot::{self, MemoryType, SearchType};
use uefi::mem::memory_map::MemoryMap;
use uefi::{runtime, system};

struct Lesson {
    title: &'static str,
    write: fn(&mut Doc),
}

const LESSONS: &[Lesson] = &[
    Lesson { title: "1  What UEFI is", write: what },
    Lesson { title: "2  From power-on to here", write: phases },
    Lesson { title: "3  The system table", write: system_table },
    Lesson { title: "4  Handles and protocols", write: handles },
    Lesson { title: "5  Boot services", write: boot_services },
    Lesson { title: "6  Runtime services", write: runtime_services },
    Lesson { title: "7  Variables", write: variables },
    Lesson { title: "8  Device paths", write: device_paths },
    Lesson { title: "9  Choosing what to boot", write: boot_manager },
    Lesson { title: "10 Images and loaders", write: images },
    Lesson { title: "11 The memory map", write: memory },
    Lesson { title: "12 The hand-off to the OS", write: handoff },
    Lesson { title: "13 Secure Boot", write: secure_boot },
];

pub fn run(scr: &mut Screen) {
    let items: Vec<Item> = LESSONS.iter().map(|l| Item::new(l.title)).collect();
    let mut state = BrowseState::default();
    loop {
        let out = tui::browse(scr, &mut state, "Tour", "Lessons", &items, &[], &mut |i, width| {
            let mut doc = Doc::new(width);
            (LESSONS[i].write)(&mut doc);
            doc
        });
        match out {
            Outcome::Back => return,
            Outcome::Open(_) => state.zoom = true,
            Outcome::Char(_) => {}
        }
    }
}

fn see(doc: &mut Doc, what: &str) {
    doc.blank();
    doc.line(
        tui::Line::of(st::GOOD, format!("{} ", tui::TRI_RIGHT)).push(st::GOOD, format!("See it: {what}")),
    );
}

fn here(doc: &mut Doc) {
    doc.h("On this machine");
}

fn what(doc: &mut Doc) {
    doc.h("Firmware with an interface");
    doc.p("When a computer is switched on, the first code to run is firmware stored in a flash chip on the board. Its job is to bring up just enough hardware (memory, a disk, a screen, a keyboard) to find an operating system and start it.");
    doc.blank();
    doc.p("UEFI, the Unified Extensible Firmware Interface, is a specification for what that firmware offers to the programs it starts. It does not say how firmware is built. It says: here is a table of functions, here is how to find devices, here is how files are named, here is the format of a program. Any OS loader written to that interface boots on any UEFI machine, whoever made the board.");
    doc.h("What it replaced");
    doc.p("The PC BIOS loaded 512 bytes from the first sector of a disk and jumped to it in 16-bit mode, and each OS found its own way from there. UEFI firmware reads a file system, loads a normal executable file from it, and runs it in the CPU's native mode with a full set of services.");
    doc.h("This program");
    doc.p("weefy is an ordinary UEFI application: a file the firmware loaded and started, exactly as it would an OS loader. Everything it shows, it gets by calling the same interface an OS loader uses. Nothing here is simulated.");
    here(doc);
    doc.kv("Firmware", String::from(system::firmware_vendor()));
    doc.kv("Its revision", format!("{:#x}", system::firmware_revision()));
    doc.kv("UEFI version", format!("{}", system::uefi_revision()));
    doc.kv("CPU", arch_name());
    see(doc, "System table");
}

fn arch_name() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "64-bit Arm (AArch64)"
    } else if cfg!(target_arch = "x86_64") {
        "64-bit x86 (x64)"
    } else {
        "other"
    }
}

fn phases(doc: &mut Doc) {
    doc.h("The phases of a boot");
    doc.p("Most UEFI firmware is built on the Platform Initialization design, which splits the boot into phases:");
    doc.blank();
    let d = |doc: &mut Doc, s: &str| doc.raw(st::KEY, s);
    d(doc, " power on");
    d(doc, "    |");
    d(doc, "   SEC   first code; a stack, a little RAM");
    d(doc, "    |");
    d(doc, "   PEI   find and train the main memory");
    d(doc, "    |");
    d(doc, "   DXE   load drivers: disks, USB, video");
    d(doc, "    |");
    d(doc, "   BDS   pick a boot option, start it");
    d(doc, "    |");
    doc.raw(st::GOOD, "   TSL   an application runs  <- you are here");
    d(doc, "    |           ExitBootServices()");
    d(doc, "   RT    the OS runs; firmware mostly gone");
    doc.blank();
    doc.bullet("SEC, Security: the reset vector. Sets up a temporary stack and hands over to PEI.");
    doc.bullet("PEI, Pre-EFI Initialization: small modules that initialize the memory controller. It ends by describing what it found in hand-off blocks (HOBs).");
    doc.bullet("DXE, Driver Execution Environment: the core that implements the boot services and runtime services, and a dispatcher that loads every driver in the firmware image. The system table exists from here on.");
    doc.bullet("BDS, Boot Device Selection: the boot manager. Connects the console and the boot devices, reads the BootOrder variable and starts the first option that works.");
    doc.bullet("TSL, Transient System Load: a UEFI application has control but the firmware's services are all still there. OS loaders live here, briefly. So does this program.");
    doc.bullet("RT, Runtime: the loader has called ExitBootServices(). The firmware's drivers are gone and the OS owns the machine. Only the runtime services remain.");
    doc.h("On Arm boards");
    doc.p("On an Arm board such as a Rockchip RK3588, earlier stages run first: a boot ROM in the chip, a loader that trains the memory, and Arm Trusted Firmware, which stays resident at the highest privilege level to handle power management (PSCI). UEFI is started by them, already with working memory, so its SEC and PEI have little to do.");
    see(doc, "System table > the HOB list and DXE services entries, which are left over from these phases");
}

fn system_table(doc: &mut Doc) {
    doc.h("One pointer to everything");
    doc.p("A UEFI application's entry point receives two arguments: a handle that stands for the image itself, and a pointer to the system table. Everything else is reached from that table:");
    doc.blank();
    doc.raw(st::KEY, " EFI_SYSTEM_TABLE");
    doc.raw(st::KEY, "   firmware vendor and revision");
    doc.raw(st::KEY, "   ConIn, ConOut, StdErr   the console");
    doc.raw(st::KEY, "   BootServices     -> table of functions");
    doc.raw(st::KEY, "   RuntimeServices  -> table of functions");
    doc.raw(st::KEY, "   ConfigurationTable[]   GUID -> pointer");
    doc.blank();
    doc.p("Each table starts with the same header: a signature, the specification revision it conforms to, its size, and a CRC32 so that a consumer can check it has not been corrupted.");
    doc.h("Configuration tables");
    doc.p("The configuration table is a list of (GUID, pointer) pairs. It is how the firmware hands over data that is not part of UEFI itself: the ACPI tables, the SMBIOS inventory, a device tree. An OS kernel looks through this list at boot for the GUIDs it knows.");
    here(doc);
    let n = system::with_config_table(|t| t.len());
    doc.kv("Config tables", format!("{n}"));
    see(doc, "System table");
}

fn handles(doc: &mut Doc) {
    doc.h("The object model");
    doc.p("UEFI has no device files and no fixed list of device types. It has a database of handles. A handle is an opaque object; what it can do is defined by the protocols installed on it.");
    doc.blank();
    doc.p("A protocol is a GUID that names an interface, plus a pointer to a structure of functions and data that implements it. A disk partition is a handle with Block I/O (read sectors), Device Path (where it is) and, if the firmware understands its format, Simple File System (open files).");
    doc.blank();
    doc.raw(st::KEY, " handle: a partition");
    doc.raw(st::KEY, "   Device Path         PciRoot/../HD(1,..)");
    doc.raw(st::KEY, "   Block I/O           ReadBlocks()");
    doc.raw(st::KEY, "   Disk I/O            ReadDisk()");
    doc.raw(st::KEY, "   Simple File System  OpenVolume()");
    doc.h("Finding things");
    doc.p("A program doesn't ask for \"the disk\". It asks the database for every handle that carries a protocol (LocateHandleBuffer), then opens the protocol on the ones it wants (OpenProtocol). This program found its keyboard, its screen and every file system that way.");
    doc.h("Drivers");
    doc.p("A UEFI driver is a program that installs a Driver Binding protocol and stays loaded. When the firmware connects a controller, it asks each driver whether it supports that handle. A driver that does opens the handle's protocols and installs new ones, often on new child handles. A USB stick becomes usable through a chain: PCI I/O, USB host controller, USB I/O, Block I/O, the partition driver's child handles, and the FAT driver's Simple File System on one of them.");
    here(doc);
    if let Ok(all) = boot::locate_handle_buffer(SearchType::AllHandles) {
        doc.kv("Handles", format!("{}", all.len()));
    }
    see(doc, "Handles, and Protocols for the same database seen the other way round");
}

fn boot_services(doc: &mut Doc) {
    doc.h("The firmware as a small OS");
    doc.p("While it is in charge, the firmware offers about forty-five functions through the boot services table. They fall into groups:");
    doc.bullet("Memory: AllocatePages, AllocatePool, GetMemoryMap. Every allocation carries a type, so the OS can later tell loader data from firmware data.");
    doc.bullet("Events and timers: CreateEvent, SetTimer, WaitForEvent. There are no threads and no interrupts for applications; everything is polling and callbacks that run at raised priority (TPL).");
    doc.bullet("Protocols: InstallProtocolInterface, OpenProtocol, LocateHandleBuffer. The handle database of the previous lesson.");
    doc.bullet("Images: LoadImage, StartImage, Exit. How one program runs another.");
    doc.bullet("Odds and ends: Stall, SetWatchdogTimer, CalculateCrc32.");
    doc.h("One CPU, no protection");
    doc.p("Boot services run on a single core, with interrupts used only for the timer tick. Applications and firmware share one address space at the same privilege level. An application can read and write anything, which is what lets this program show you the tables directly.");
    doc.h("They end");
    doc.p("ExitBootServices() is the last boot service an OS loader calls. After it, the whole table is invalid: its memory belongs to the OS. That is why they are boot services.");
    doc.h("The watchdog");
    doc.p("Before it starts a boot option, the boot manager arms a five-minute watchdog. A loader that hangs gets the machine reset. This program switched it off when it started, since you may well read for longer than five minutes, and arms it again before it starts your OS.");
    see(doc, "Boot services, where each function is listed with the address this firmware put in the table");
}

fn runtime_services(doc: &mut Doc) {
    doc.h("What survives");
    doc.p("A few services stay available after the OS has taken over. Their code and data sit in memory marked as runtime in the memory map, which the OS must leave alone and map into its own address space.");
    doc.bullet("Variables: GetVariable, SetVariable, GetNextVariableName. How an OS installer registers its boot option, and how efibootmgr reads it back.");
    doc.bullet("Time: GetTime and SetTime on the real-time clock.");
    doc.bullet("ResetSystem: reboot or power off.");
    doc.bullet("UpdateCapsule: hand a firmware update to the firmware, to be applied on the next reset.");
    doc.bullet("SetVirtualAddressMap: called once by the OS to tell the firmware where its runtime memory now appears, so that its internal pointers can be fixed up.");
    doc.h("Less than it sounds");
    doc.p("At runtime the OS is running its own drivers on the hardware, so a runtime service can't simply use a device. On many Arm boards the variable store is on the same flash or SD card the OS drives, and SetVariable at runtime returns an error. A configuration table (the RT properties table) says which services really work.");
    here(doc);
    match runtime::get_time() {
        Ok(t) => doc.kv(
            "GetTime()",
            format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", t.year(), t.month(), t.day(), t.hour(), t.minute(), t.second()),
        ),
        Err(e) => doc.kv("GetTime()", format!("{:?}", e.status())),
    }
    see(doc, "Runtime services");
}

fn variables(doc: &mut Doc) {
    doc.h("The firmware's settings store");
    doc.p("UEFI variables are named blobs of bytes. A variable is identified by its name and a vendor GUID, the namespace that keeps one vendor's \"Timeout\" apart from another's. The specification's own variables live in the Global Variable namespace.");
    doc.h("Attributes");
    doc.bullet("NV, non-volatile: kept in flash across resets. Without it, the variable lives in RAM until power-off.");
    doc.bullet("BS, boot-service access: readable before ExitBootServices().");
    doc.bullet("RT, runtime access: also readable from the running OS.");
    doc.bullet("AT, authenticated: a write must be signed by the right key. The Secure Boot databases are protected this way.");
    doc.p("A variable with BS but not RT is hidden from the OS. Firmware uses that for things an OS must not tamper with.");
    doc.h("The ones that matter for booting");
    doc.bullet("Boot0000, Boot0001, ...: one boot option each: a description, a device path, optional data.");
    doc.bullet("BootOrder: the option numbers, in the order to try them.");
    doc.bullet("BootNext: boot this option once, on the next boot only.");
    doc.bullet("BootCurrent: the option that was booted this time.");
    doc.bullet("Timeout: seconds the boot manager waits for a key.");
    doc.bullet("OsIndications: requests from the OS, such as \"enter firmware setup on the next boot\".");
    doc.p("On Linux these appear under /sys/firmware/efi/efivars, and efibootmgr edits them.");
    here(doc);
    let count = runtime::variable_keys().filter(|k| k.is_ok()).count();
    doc.kv("Variables", format!("{count}"));
    let order = bootopt::boot_order();
    doc.kv(
        "BootOrder",
        if order.is_empty() {
            String::from("not set")
        } else {
            order.iter().map(|n| format!("{n:04X}")).collect::<Vec<_>>().join(" ")
        },
    );
    doc.dim("This program only reads variables, with one exception: it sets BootCurrent when it starts a boot option for you, as a boot manager does.");
    see(doc, "Variables");
}

fn device_paths(doc: &mut Doc) {
    doc.h("Addresses for devices");
    doc.p("A device path says where something is, as a list of nodes from the root of the machine outwards. Each node is a small binary structure with a type, a subtype and a length. In text:");
    doc.blank();
    doc.raw(st::KEY, " PciRoot(0x0)/Pci(0x1,0x0)/");
    doc.raw(st::KEY, "   NVMe(0x1,00-00-..)/");
    doc.raw(st::KEY, "   HD(1,GPT,5a3f..,0x800,0x100000)/");
    doc.raw(st::KEY, "   \\EFI\\fedora\\shimaa64.efi");
    doc.blank();
    doc.p("Read left to right: a PCI root bridge, a device on it, an NVMe namespace, partition 1 of a GPT disk with its unique GUID, and a file on that partition.");
    doc.h("Node types");
    doc.bullet("Hardware: PCI devices, memory-mapped devices, vendor-defined.");
    doc.bullet("ACPI: devices named by their ACPI ID, such as PciRoot.");
    doc.bullet("Messaging: how to talk to it: USB port, SATA port, NVMe namespace, MAC address, IPv4, URI.");
    doc.bullet("Media: what is on it: a partition (HD), a CD-ROM entry, a file path, a file in a firmware volume.");
    doc.bullet("End: every path ends with one.");
    doc.h("Short forms");
    doc.p("A boot option usually stores only HD(...)/file. The boot manager finds the partition with that GUID wherever the disk is plugged in, and fills in the front of the path. That is why moving a disk to another slot doesn't break booting.");
    here(doc);
    if let Some(image) = util::open::<uefi::proto::loaded_image::LoadedImage>(boot::image_handle()) {
        if let Some(dev) = image.device().and_then(util::handle_dp) {
            doc.kv("Loaded from", util::dp_bytes_text(&dev));
        }
        if let Some(file) = image.file_path() {
            doc.kv("File", util::dp_text(file));
        }
    }
    see(doc, "Handles: most have a device path. Boot: each option shows its stored path and what it resolves to");
}

fn boot_manager(doc: &mut Doc) {
    doc.h("The boot manager's algorithm");
    doc.p("The BDS phase runs a simple loop that the specification defines:");
    doc.bullet("If BootNext is set, delete it and try that option first.");
    doc.bullet("Then try each option in BootOrder. For each: read Boot####, resolve its device path to a file, LoadImage() it, StartImage() it.");
    doc.bullet("If the image returns, or can't be loaded, go on to the next option.");
    doc.bullet("If nothing in the list works, look for the default loader on every device.");
    doc.h("The default loader");
    doc.p("A disk that has never been registered in BootOrder still boots if it has a FAT partition with a file at a fixed, architecture-specific name:");
    doc.raw(st::KEY, format!("  {}", bootopt::default_loader()));
    doc.p("This is how installation media and removable disks boot, and it is the simplest way to make a program like this one run at boot: put it on a FAT partition under that name.");
    doc.h("The EFI System Partition");
    doc.p("Loaders live on the ESP: a FAT partition whose partition type GUID marks it as such. Each OS keeps its loader in its own directory, \\EFI\\<vendor>\\, and registers a Boot#### option pointing to it. Several systems share one ESP without overwriting each other.");
    doc.h("When an option returns");
    doc.p("A boot option is a function call: StartImage() returns when the image exits. If it exits with an error, the boot manager tries the next option. If it exits with success, edk2-based firmware shows its own menu. This program's Leave screen uses both.");
    here(doc);
    let targets = bootopt::targets();
    match bootopt::global_u16("BootCurrent") {
        Some(n) => doc.kv("BootCurrent", format!("Boot{n:04X}")),
        None => doc.kv("BootCurrent", "not set"),
    }
    doc.kv("Boot options", format!("{}", targets.iter().filter(|t| matches!(t.source, bootopt::Source::Entry(_))).count()));
    doc.kv("Found on disk", format!("{}", targets.iter().filter(|t| t.source == bootopt::Source::File).count()));
    see(doc, "Boot");
}

fn images(doc: &mut Doc) {
    doc.h("One file format");
    doc.p("Everything UEFI runs is a PE/COFF image, the executable format of Windows, whatever the CPU. The firmware's own drivers, option ROMs on cards, the UEFI Shell, GRUB, the Windows boot manager and a Linux kernel built with the EFI stub are all files of this kind. A field in the header says which:");
    doc.bullet("Application: runs, returns, and is unloaded. OS loaders are applications that never return, because they call ExitBootServices().");
    doc.bullet("Boot service driver: stays loaded until ExitBootServices().");
    doc.bullet("Runtime driver: stays loaded for the life of the OS; it implements runtime services.");
    doc.h("Loading and starting");
    doc.p("LoadImage() reads the file, checks its signature if Secure Boot is on, relocates it and creates a handle for it with a Loaded Image protocol. StartImage() calls its entry point with that handle and the system table. The image finds out where it came from by opening Loaded Image on its own handle.");
    doc.h("The usual chain on Linux");
    doc.raw(st::KEY, " firmware -> shim -> GRUB or systemd-boot");
    doc.raw(st::KEY, "          -> kernel (EFI stub) -> kernel");
    doc.p("Each arrow is LoadImage() and StartImage(), or something like it. The kernel's EFI stub is the part that finally calls ExitBootServices().");
    here(doc);
    if let Some(image) = util::open::<uefi::proto::loaded_image::LoadedImage>(boot::image_handle()) {
        let (base, size) = image.info();
        doc.kv("This image at", format!("{:#x}", base as usize));
        doc.kv("Its size", util::size(size));
    }
    see(doc, "Handles > search for LoadedImage (press / and type it) to see every image in memory");
}

fn memory(doc: &mut Doc) {
    doc.h("Who owns which page");
    doc.p("The firmware keeps a map of all physical memory as a list of ranges, each with a type. GetMemoryMap() returns it. An OS builds its own memory management from this list and nothing else.");
    doc.h("The types that matter");
    doc.bullet("Conventional: free. The OS may use it.");
    doc.bullet("Loader code and data: the OS loader and what it allocated. The kernel, the initrd and the command line are handed over in memory of these types.");
    doc.bullet("Boot services code and data: the firmware's drivers and their data. Free for the OS once it has taken over.");
    doc.bullet("Runtime services code and data: the part of the firmware that stays. The OS must preserve it and map it.");
    doc.bullet("ACPI reclaim: the ACPI tables. Free once the OS has read them.");
    doc.bullet("ACPI NVS and reserved: never touch.");
    doc.bullet("Memory-mapped I/O: device registers the runtime services need.");
    doc.h("ExitBootServices and the map key");
    doc.p("Each call to GetMemoryMap() returns a key that changes whenever the map changes. ExitBootServices() takes that key. If anything allocated memory in between, the key is stale and the call fails, so loaders get the map and exit in a tight loop. It is the firmware's way of making sure the OS has an exact picture of memory at the moment it takes over.");
    here(doc);
    if let Ok(map) = boot::memory_map(MemoryType::LOADER_DATA) {
        let total: u64 = map
            .entries()
            .filter(|d| d.ty != MemoryType::MMIO && d.ty != MemoryType::MMIO_PORT_SPACE && d.ty != MemoryType::RESERVED)
            .map(|d| d.page_count * 4096)
            .sum();
        let free: u64 = map.entries().filter(|d| d.ty == MemoryType::CONVENTIONAL).map(|d| d.page_count * 4096).sum();
        doc.kv("Map entries", format!("{}", map.len()));
        doc.kv("Memory", util::size(total));
        doc.kv("Free", util::size(free));
    }
    see(doc, "Memory map");
}

fn handoff(doc: &mut Doc) {
    doc.h("What an OS takes with it");
    doc.p("When the loader calls ExitBootServices(), the firmware's drivers stop and its boot services vanish. The OS keeps:");
    doc.bullet("The memory map, as of that instant.");
    doc.bullet("The system table pointer, for the runtime services and the configuration tables.");
    doc.bullet("The hardware description: ACPI tables or a device tree, found through the configuration tables.");
    doc.bullet("SMBIOS: the inventory that tools like dmidecode print.");
    doc.bullet("The framebuffer: the address and layout the Graphics Output protocol reported, so that early kernel messages can be drawn without a graphics driver.");
    doc.bullet("Whatever the loader read while it still could: the kernel, the initrd, random seed, the TPM event log.");
    doc.h("ACPI or device tree");
    doc.p("An OS can't discover most of an Arm board's hardware by probing; it must be told. PCs and Arm servers use ACPI tables for that. Embedded Arm boards traditionally use a device tree. Firmware for boards like the RK3588 ones often offers both and lets you choose in its setup menu; the configuration table shows which one this boot is offering.");
    here(doc);
    system::with_config_table(|tables| {
        use uefi::table::cfg::ConfigTableEntry as C;
        let has = |g: uefi::Guid| tables.iter().any(|t| t.guid == g);
        let yn = |b: bool| if b { "present" } else { "absent" };
        doc.kv("ACPI", yn(has(C::ACPI2_GUID) || has(C::ACPI_GUID)));
        doc.kv("Device tree", yn(has(uefi::guid!("b1b621d5-f19c-41a5-830b-d9152c69aae0"))));
        doc.kv("SMBIOS", yn(has(C::SMBIOS3_GUID) || has(C::SMBIOS_GUID)));
    });
    see(doc, "System table > the configuration table entries");
}

fn secure_boot(doc: &mut Doc) {
    doc.h("Only run what is signed");
    doc.p("With Secure Boot on, LoadImage() refuses any image that isn't vouched for. The rules are held in authenticated variables:");
    doc.bullet("PK, the Platform Key: one key, the machine owner's (in practice the manufacturer's). It authorizes changes to KEK.");
    doc.bullet("KEK, Key Exchange Keys: authorize changes to db and dbx.");
    doc.bullet("db: certificates and hashes that are allowed to run.");
    doc.bullet("dbx: certificates and hashes that are revoked. It wins over db.");
    doc.p("An image runs if its signature chains to a certificate in db, or its hash is in db, and nothing about it is in dbx.");
    doc.h("Setup mode");
    doc.p("With no PK enrolled, the firmware is in setup mode: Secure Boot is off and anyone can write the key variables. Enrolling a PK switches to user mode. The SetupMode and SecureBoot variables report the state.");
    doc.h("Linux and shim");
    doc.p("Most PCs ship with Microsoft's certificates in db. Linux distributions boot through shim, a small loader Microsoft has signed, which then checks GRUB and the kernel against the distribution's own key and against keys the owner enrolls (MOK).");
    doc.h("Measured boot");
    doc.p("Secure Boot refuses; measured boot records. With a TPM, each stage hashes the next into the TPM's registers before running it, and secrets such as a disk encryption key can be sealed to a known-good sequence.");
    here(doc);
    let flag = |name: &str| match bootopt::global_var(name) {
        Some((d, _)) if d.first() == Some(&1) => "1 (yes)",
        Some(_) => "0 (no)",
        None => "variable absent",
    };
    doc.kv("SecureBoot", flag("SecureBoot"));
    doc.kv("SetupMode", flag("SetupMode"));
    doc.dim("A program like this one must be signed with a key in db, or Secure Boot must be off, for the firmware to run it at all.");
    see(doc, "Secure Boot lab (main menu)");
}
