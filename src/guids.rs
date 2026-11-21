//! Names and explanations for the GUIDs a machine shows.
//!
//! UEFI names everything with a 128-bit GUID: protocols, configuration
//! tables, variable namespaces, partition types. The names come from edk2's
//! package declarations (guid_names.rs); the explanations are written here
//! for the ones worth knowing.

use crate::guid_names::NAMES;
use alloc::format;
use alloc::string::String;
use uefi::Guid;

/// The name edk2 gives a GUID, and the package that declares it.
pub fn lookup(guid: &Guid) -> Option<(&'static str, &'static str)> {
    NAMES.iter().find(|e| e.0 == *guid).map(|e| (e.1, e.2))
}

/// A short name for a GUID: edk2's without its Efi prefix and Protocol
/// suffix, or the GUID itself if it has none.
pub fn short(guid: &Guid) -> String {
    match lookup(guid) {
        Some((name, _)) => {
            let name = name.strip_prefix("Efi").unwrap_or(name);
            let name = name.strip_suffix("Protocol").unwrap_or(name);
            String::from(name)
        }
        None => format!("{guid}"),
    }
}

/// What a GUID is, for the ones this program explains.
pub fn about(guid: &Guid) -> Option<&'static str> {
    let (name, _) = lookup(guid)?;
    ABOUT.iter().find(|e| e.0 == name).map(|e| e.1)
}

const ABOUT: &[(&str, &str)] = &[
    // Images and device paths
    ("EfiLoadedImageProtocol", "Installed on the handle of every loaded image (application, driver, OS loader). It records where the image came from, where it sits in memory and the options it was started with. An OS loader reads its own to find its command line and the disk it was loaded from."),
    ("EfiLoadedImageDevicePathProtocol", "The full device path an image was loaded from, installed beside Loaded Image on the image's handle."),
    ("EfiDevicePathProtocol", "The handle's address: a list of nodes that say how to reach the device from the root of the machine, such as PciRoot/Pci/NVMe/HD/file. Boot options store device paths, and that is how the firmware finds the same disk on the next boot."),
    ("EfiDevicePathToTextProtocol", "Converts device paths to the text form you see in this program, e.g. PciRoot(0x0)/Pci(0x1,0x0)/..."),
    ("EfiDevicePathFromTextProtocol", "Parses the text form of a device path back into its binary form."),
    ("EfiDevicePathUtilitiesProtocol", "Helpers for building and joining device paths."),
    // Console
    ("EfiSimpleTextInProtocol", "Keyboard input: ReadKeyStroke() and an event that fires when a key is waiting. This program reads its keys through it."),
    ("EfiSimpleTextInputExProtocol", "Keyboard input with modifier keys (Shift, Ctrl, Alt) and hot-key notifications."),
    ("EfiSimpleTextOutProtocol", "Text output: a grid of characters with 16 foreground and 8 background colours. Everything this program draws goes through OutputString(), SetAttribute() and SetCursorPosition() of this protocol."),
    ("EfiGraphicsOutputProtocol", "GOP: the framebuffer. It lists the video modes, switches between them and can copy pixels (Blt). An OS loader takes the framebuffer address from here so the kernel can draw before it has a graphics driver."),
    ("EfiEdidDiscoveredProtocol", "The EDID block read from the attached display: what resolutions it supports."),
    ("EfiEdidActiveProtocol", "The EDID block the firmware is actually using for the display."),
    ("EfiSerialIoProtocol", "A serial port: baud rate, control lines, Read() and Write(). The firmware's terminal driver sits on top of it to make a text console."),
    ("EfiSimplePointerProtocol", "A mouse or other relative pointing device."),
    ("EfiAbsolutePointerProtocol", "A touchscreen or other absolute pointing device."),
    ("EfiHiiDatabaseProtocol", "The Human Interface Infrastructure database: the forms, strings and fonts that firmware setup screens are built from."),
    ("EfiHiiStringProtocol", "Looks up localized strings in the HII database."),
    ("EfiHiiFontProtocol", "Renders text with the fonts in the HII database."),
    ("EfiHiiConfigRoutingProtocol", "Routes setup-screen settings to the drivers that own them."),
    ("EfiHiiConfigAccessProtocol", "A driver's side of a setup screen: it reads and writes the settings the screen shows."),
    ("EfiFormBrowser2Protocol", "The setup browser: draws HII forms and lets the user change settings."),
    // Storage
    ("EfiBlockIoProtocol", "A block device: ReadBlocks() and WriteBlocks() on a disk or a partition. The firmware installs one for each whole disk and one for each partition it finds on it."),
    ("EfiBlockIo2Protocol", "Block I/O that doesn't wait: requests complete through events."),
    ("EfiDiskIoProtocol", "Byte-addressed access on top of Block I/O, used by file system drivers so they need not think in blocks."),
    ("EfiDiskIo2Protocol", "Disk I/O that doesn't wait."),
    ("EfiSimpleFileSystemProtocol", "A file system the firmware can read (FAT always; others if a driver is loaded). OpenVolume() returns the root directory. This is how the firmware reads \\EFI\\BOOT\\BOOTAA64.EFI from the EFI System Partition, and how an OS loader reads its kernel."),
    ("EfiPartitionInfoProtocol", "What kind of partition this handle is (GPT or MBR) and its partition table entry."),
    ("EfiLoadFileProtocol", "A device the firmware can boot from by asking it for a file, rather than by reading a file system: network boot (PXE, HTTP) works this way."),
    ("EfiLoadFile2Protocol", "Like Load File, for files that are not boot options. Linux uses one to receive its initrd from the loader."),
    ("EfiScsiIoProtocol", "Sends SCSI commands to one SCSI device."),
    ("EfiExtScsiPassThruProtocol", "Sends SCSI commands through a SCSI controller to any device on it."),
    ("EfiAtaPassThruProtocol", "Sends ATA commands through a SATA or IDE controller."),
    ("EfiNvmExpressPassThruProtocol", "Sends NVMe commands through an NVMe controller."),
    ("EfiSdMmcPassThruProtocol", "Sends SD and eMMC commands through an SD host controller."),
    ("EfiStorageSecurityCommandProtocol", "Security commands for self-encrypting drives (TCG Opal)."),
    ("EfiEraseBlockProtocol", "Erases ranges of blocks on a device (TRIM, discard)."),
    ("EfiFirmwareVolume2Protocol", "A firmware volume: the file system inside the firmware image itself, where its drivers and built-in applications are stored."),
    ("EfiFirmwareVolumeBlockProtocol", "Block access to the flash that holds a firmware volume or the variable store."),
    ("EfiUnicodeCollationProtocol", "Case-insensitive string comparison, which the FAT driver needs for file names."),
    ("EfiUnicodeCollation2Protocol", "Case-insensitive string comparison, which the FAT driver needs for file names."),
    // Buses
    ("EfiPciRootBridgeIoProtocol", "A PCI host bridge: access to the configuration space, memory and I/O of every device behind it."),
    ("EfiPciIoProtocol", "One PCI device: its configuration space, its BARs and DMA mapping. Device drivers bind to handles that carry this."),
    ("EfiUsb2HcProtocol", "A USB host controller (xHCI, EHCI)."),
    ("EfiUsbIoProtocol", "One USB interface of one device. USB keyboard and storage drivers bind to handles that carry this."),
    ("EfiI2cMasterProtocol", "An I2C controller."),
    ("EfiCpuIo2Protocol", "Raw memory-mapped and port I/O."),
    ("EfiCpuArchProtocol", "The CPU as the firmware core sees it: interrupts on and off, exception handlers, cache flushing, memory attributes."),
    ("EfiTimerArchProtocol", "The timer tick that drives the firmware's events."),
    ("EfiMpServiceProtocol", "The other CPU cores: how many there are, and a way to run a function on them. The firmware itself runs on one core."),
    ("HardwareInterruptProtocol", "The interrupt controller, as the firmware's drivers use it."),
    ("HardwareInterrupt2Protocol", "The interrupt controller, as the firmware's drivers use it."),
    // Drivers
    ("EfiDriverBindingProtocol", "The heart of a UEFI driver: Supported() says whether it can drive a given handle, Start() makes it do so and Stop() undoes it. ConnectController() walks every one of these to match drivers to devices."),
    ("EfiComponentNameProtocol", "A human-readable name for a driver and for the devices it manages."),
    ("EfiComponentName2Protocol", "A human-readable name for a driver and for the devices it manages."),
    ("EfiDriverDiagnostics2Protocol", "Self-tests a driver offers for its devices."),
    ("EfiPlatformDriverOverrideProtocol", "Lets the platform choose which driver binds to a device."),
    ("EfiBusSpecificDriverOverrideProtocol", "Lets a bus choose a driver for a device, such as a PCI option ROM."),
    ("EfiDriverSupportedEfiVersionProtocol", "The UEFI version a driver was written for."),
    // Network
    ("EfiSimpleNetworkProtocol", "A network card at its simplest: send a frame, receive a frame. The network stack is built on top of it."),
    ("EfiManagedNetworkProtocol", "Shared, queued access to a network card."),
    ("EfiManagedNetworkServiceBindingProtocol", "Creates Managed Network instances on a network card."),
    ("EfiPxeBaseCodeProtocol", "PXE: DHCP and TFTP for booting from the network."),
    ("EfiIp4Config2Protocol", "The IPv4 configuration of an interface."),
    ("EfiIp4ServiceBindingProtocol", "Creates IPv4 instances on an interface."),
    ("EfiTcp4ServiceBindingProtocol", "Creates TCP connections over IPv4."),
    ("EfiUdp4ServiceBindingProtocol", "Creates UDP sockets over IPv4."),
    ("EfiDhcp4ServiceBindingProtocol", "Creates DHCP clients on an interface."),
    ("EfiArpServiceBindingProtocol", "Creates ARP instances on an interface."),
    ("EfiHttpServiceBindingProtocol", "Creates HTTP clients: the base of HTTP boot."),
    ("EfiDns4ServiceBindingProtocol", "Creates DNS resolvers."),
    ("EfiTlsServiceBindingProtocol", "Creates TLS sessions, for HTTPS boot."),
    // Security
    ("EfiRngProtocol", "Random numbers from the platform's entropy source. Linux asks for some at boot to seed its own generator and to randomize the kernel's address."),
    ("EfiTcg2Protocol", "The TPM 2.0: measure data into its registers, read the event log. Each stage of a measured boot records the next one here."),
    ("EfiTcgProtocol", "The TPM 1.2."),
    ("EfiSecurityArchProtocol", "The firmware core calls this before it runs any image; Secure Boot's signature check is hooked in here."),
    ("EfiSecurity2ArchProtocol", "The firmware core calls this before it runs any image; Secure Boot's signature check is hooked in here."),
    ("EfiHashServiceBindingProtocol", "Creates hash instances."),
    ("EfiHash2ServiceBindingProtocol", "Creates hash instances."),
    ("EfiPkcs7VerifyProtocol", "Verifies PKCS#7 signatures against lists of trusted and revoked certificates."),
    ("EfiMemoryAttributeProtocol", "Changes page permissions (read-only, no-execute). Loaders use it so that their code and data are not writable and executable at once."),
    // Architecture and system
    ("EfiRuntimeArchProtocol", "The list of runtime images and events the firmware core must relocate when the OS calls SetVirtualAddressMap()."),
    ("EfiVariableArchProtocol", "Says that reading variables works. A marker with no functions."),
    ("EfiVariableWriteArchProtocol", "Says that writing variables works. A marker with no functions."),
    ("EfiBdsArchProtocol", "The Boot Device Selection phase: the code that reads BootOrder and starts boot options. The DXE core calls its one function when every driver is loaded."),
    ("EfiWatchdogTimerArchProtocol", "The timer behind SetWatchdogTimer()."),
    ("EfiRealTimeClockArchProtocol", "Says that GetTime() and SetTime() work."),
    ("EfiResetArchProtocol", "Says that ResetSystem() works."),
    ("EfiMonotonicCounterArchProtocol", "Says that the monotonic counter works."),
    ("EfiCapsuleArchProtocol", "Says that UpdateCapsule() works."),
    ("EfiMetronomeArchProtocol", "A fixed-period tick used for short delays (Stall)."),
    ("EfiStatusCodeRuntimeProtocol", "Progress and error codes reported by firmware components, such as the POST codes on a debug display."),
    ("EfiDecompressProtocol", "The UEFI compression algorithm's decompressor."),
    ("EfiEbcProtocol", "The EFI Byte Code interpreter, for drivers compiled to run on any CPU."),
    ("EfiAcpiTableProtocol", "Installs and removes ACPI tables. Drivers publish their tables through it before the OS boots."),
    ("EfiAcpiSdtProtocol", "Walks and edits the installed ACPI tables."),
    ("EfiSmbiosProtocol", "Adds and reads SMBIOS records: the machine's inventory (vendor, model, memory modules)."),
    ("EfiFirmwareManagementProtocol", "A firmware component that can be updated: its version, and a way to write a new image."),
    ("EfiShellProtocol", "The UEFI Shell's services, present while the shell is running."),
    ("EfiShellParametersProtocol", "The command line of a program started from the UEFI Shell."),
    ("EfiTimestampProtocol", "A high-resolution counter for measuring time."),
    ("EfiResetNotificationProtocol", "Lets drivers hear about a reset before it happens."),
    ("EfiDeferredImageLoadProtocol", "Images whose loading was put off, such as option ROMs waiting for a user to be present."),
    ("EfiBootManagerPolicyProtocol", "Asks the boot manager to connect devices: all of them, the console, or the network."),
    ("EdkiiVariableLockProtocol", "Makes a variable read-only until the next reset."),
    ("EdkiiVariablePolicyProtocol", "Rules for which variables may be created or changed, and how."),
    // Configuration tables
    ("EfiAcpiTable", "The ACPI root pointer (RSDP, ACPI 2.0 or later). It leads to every ACPI table: the description of the hardware the OS can't discover on its own, and the power management methods."),
    ("EfiAcpi20Table", "The ACPI root pointer (RSDP, ACPI 2.0 or later). It leads to every ACPI table: the description of the hardware the OS can't discover on its own, and the power management methods."),
    ("EfiAcpi10Table", "The ACPI 1.0 root pointer, kept for old operating systems."),
    ("EfiSmbiosTable", "The SMBIOS 2.x entry point (32-bit): the machine's inventory."),
    ("EfiSmbios3Table", "The SMBIOS 3.x entry point (64-bit): the machine's inventory. dmidecode on Linux prints what this leads to."),
    ("FdtTable", "A flattened device tree: the description of the hardware that Arm and RISC-V systems use in place of, or beside, ACPI."),
    ("EfiDxeServicesTable", "The DXE core's own services (memory space map, dispatcher), from the Platform Initialization specification. Firmware drivers use it; an OS has no need for it."),
    ("EfiHobList", "The hand-off blocks the early firmware phases passed to DXE: what memory was found, what firmware volumes exist."),
    ("EfiMemoryTypeInformation", "How much memory of each type the firmware reserves up front so that the memory map keeps the same shape from boot to boot, which hibernation needs."),
    ("EfiMemoryAttributesTable", "Page permissions for the runtime services' memory: which parts are code (read-only) and which are data (not executable), so the OS can map them strictly."),
    ("EfiDebugImageInfoTable", "A list of every loaded image and its address, for debuggers."),
    ("EfiSystemResourceTable", "ESRT: the firmware components that can be updated with capsules, and their versions. fwupd on Linux reads this."),
    ("EfiImageSecurityDatabase", "The namespace of Secure Boot's signature databases: db (allowed), dbx (revoked), dbt and dbr."),
    ("EfiRtPropertiesTable", "Which runtime services still work after the OS has taken over. Firmware on small boards often can't write variables at runtime, and says so here."),
    ("EfiConfProfilesTable", "Which UEFI conformance profiles the firmware claims, such as the reduced profile for embedded boards (EBBR)."),
    ("EfiTcg2FinalEventsTable", "TPM measurements made after the OS loader fetched the event log."),
    ("LzmaCustomDecompress", "Says the firmware can decompress LZMA; used inside the firmware image."),
    ("EfiMemoryStatusCodeRecord", "A log of firmware status codes kept in memory."),
    ("EfiCertX509", "An X.509 certificate: a signer Secure Boot trusts (in db) or has revoked (in dbx)."),
    ("EfiCertSha256", "A SHA-256 hash of one image: allowed (in db) or revoked (in dbx)."),
    // Variable namespaces
    ("MtcVendor", "The namespace of the variable that keeps the high half of the monotonic counter across resets."),
    ("EfiConfProfilesUefiSpec", "The full UEFI specification profile."),
    ("EfiConfProfilesEbbrSpec21", "EBBR 2.1: the reduced set of UEFI that embedded boards must provide."),
    ("EfiConfProfilesEbbrSpec22", "EBBR 2.2: the reduced set of UEFI that embedded boards must provide."),
    ("EfiGlobalVariable", "The namespace of the variables the UEFI specification defines: BootOrder, Boot####, Timeout, SecureBoot, PlatformLang and the rest."),
];

/// The Linux boot loader's own configuration tables and other well-known
/// GUIDs that no edk2 package declares.
pub fn extra(guid: &Guid) -> Option<(&'static str, &'static str)> {
    use uefi::guid;
    const EXTRA: &[(Guid, &str, &str)] = &[
        (guid!("4a67b082-0a4c-41cf-b6c7-440b29bb8c4f"), "systemd / Boot Loader Interface", "The namespace systemd-boot and other loaders use to tell the OS which entry was booted and how long it took (bootctl reads these)."),
        (guid!("605dab50-e046-4300-abb6-3dd810dd8b23"), "shim", "The namespace of shim, the small signed loader Linux distributions use under Secure Boot. MokList holds the machine owner's own keys."),
        (guid!("1ce1e5bc-7ceb-42f2-81e5-8aadf180f57b"), "Linux random seed table", "Random bytes a loader leaves for the Linux kernel to seed its random number generator."),
        (guid!("5568e427-68fc-4f3d-ac74-ca555231cc68"), "Linux initrd media", "The device path on which a loader offers Linux its initrd through Load File 2."),
        (guid!("b7799cb0-eca2-4943-9667-1fae07b747fa"), "Linux TPM event log table", "The TPM event log, copied by the Linux EFI stub before it leaves boot services."),
        (guid!("888eb0c6-8ede-4ff5-a8f0-9aee5cb977c2"), "Linux memreserve table", "Memory the Linux kernel must keep reserved across kexec."),
        (guid!("c451ed2b-9694-45d3-baba-ed9f8988a389"), "Linux MOK variable table", "shim's machine owner keys, passed to the Linux kernel as a table."),
        (guid!("77fa9abd-0359-4d32-bd60-28f4e78f784b"), "Microsoft", "Microsoft's owner GUID: it marks the Secure Boot certificates Microsoft supplied."),
    ];
    EXTRA.iter().find(|e| e.0 == *guid).map(|e| (e.1, e.2))
}

/// The best name for a GUID with the GUID after it, for detail pages.
pub fn full(guid: &Guid) -> String {
    match lookup(guid).map(|n| n.0).or(extra(guid).map(|n| n.0)) {
        Some(name) => format!("{name}  {guid}"),
        None => format!("{guid}"),
    }
}

/// The explanation for a GUID from either table.
pub fn explain(guid: &Guid) -> Option<&'static str> {
    about(guid).or(extra(guid).map(|e| e.1))
}
