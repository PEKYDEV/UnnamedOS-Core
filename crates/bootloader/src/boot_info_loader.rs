#[cfg(feature = "exit-boot-services-test")]
use core::convert::Infallible;
use core::{mem::size_of, ptr::NonNull};

use boot_protocol::{
    BOOT_INFO_SIZE, MEMORY_DESCRIPTOR_SIZE, MEMORY_KIND_BOOT_INFO, MEMORY_KIND_BOOT_MEMORY_MAP,
    MEMORY_KIND_BOOTSTRAP_STACK, MEMORY_KIND_FRAMEBUFFER, MEMORY_KIND_KERNEL_IMAGE,
    MemoryDescriptor,
};
#[cfg(feature = "kernel-handoff-test")]
use bootloader::{BOOTSTRAP_IDENTITY_LIMIT, BOOTSTRAP_STACK_CANARY};
#[cfg(feature = "exit-boot-services-test")]
use bootloader::{
    BOOTSTRAP_STACK_PAGES, BootServicesState, ExitRetryPolicy, FinalMapMetadata, PostExitState,
    TransferredBootState, TransferredBootstrapStack, map_uefi_memory_kind_post_exit,
};
use bootloader::{
    BootDataAllocations, CONVERTED_DESCRIPTOR_CAPACITY, GopFramebuffer, GopPixelFormat,
    LoadedKernel, MapBuildError, MapKeyStatus, PageAllocation, PageBackend,
    PageTableReservationProof, PreparedBootInfo, ProvisionalMapMetadata, RAW_MEMORY_MAP_MAX_BYTES,
    Reservation, ReservationList, ReservationSource, append_page_table_reservations,
    apply_reservations, build_boot_info, convert_framebuffer, map_uefi_memory_kind,
    normalize_mapped_memory_map, retry_is_allowed,
};
#[cfg(feature = "kernel-handoff-test")]
use bootloader::{BootstrapStack, PhysicalRange};
use uefi::{
    Status,
    boot::{self, AllocateType, MemoryType},
    mem::memory_map::MemoryDescriptor as UefiMemoryDescriptor,
    proto::console::gop::{GraphicsOutput, PixelFormat},
};

#[cfg(feature = "kernel-handoff-test")]
use crate::page_table_loader::{self, PageTableLoadError};
use crate::serial::SerialPort;
use memory_layout::PhysicalFrame;

const RAW_MAP_PAGES: usize = RAW_MEMORY_MAP_MAX_BYTES / 4096;
const CONVERTED_MAP_BYTES: usize = CONVERTED_DESCRIPTOR_CAPACITY * 32;
const CONVERTED_MAP_PAGES: usize = CONVERTED_MAP_BYTES / 4096;
const BOOT_INFO_PAGES: usize = 1;

const GOP_READY_MARKER: &[u8] = b"UNOS:P1G:GOP_READY";
const BUFFERS_READY_MARKER: &[u8] = b"UNOS:P1G:BUFFERS_READY";
const MAP_CAPTURED_MARKER: &[u8] = b"UNOS:P1G:MAP_CAPTURED";
const MAP_CONVERTED_MARKER: &[u8] = b"UNOS:P1G:MAP_CONVERTED";
const RESERVATIONS_VALID_MARKER: &[u8] = b"UNOS:P1G:RESERVATIONS_VALID";
const BOOTINFO_VALID_MARKER: &[u8] = b"UNOS:P1G:BOOTINFO_VALID";
const OWNERSHIP_READY_MARKER: &[u8] = b"UNOS:P1G:OWNERSHIP_READY";
#[cfg(not(feature = "exit-boot-services-test"))]
const MEMORY_RELEASED_MARKER: &[u8] = b"UNOS:P1G:MEMORY_RELEASED";
#[cfg(not(feature = "exit-boot-services-test"))]
const PASS_MARKER: &[u8] = b"UNOS:P1G:PASS";
#[cfg(feature = "exit-boot-services-test")]
const EXIT_READY_MARKER: &[u8] = b"UNOS:P1H:EXIT_READY";
#[cfg(feature = "exit-boot-services-test")]
const BOOT_SERVICES_EXITED_MARKER: &[u8] = b"UNOS:P1H:BOOT_SERVICES_EXITED";
#[cfg(feature = "exit-boot-services-test")]
const FINAL_MAP_CONVERTED_MARKER: &[u8] = b"UNOS:P1H:FINAL_MAP_CONVERTED";
#[cfg(feature = "exit-boot-services-test")]
const BOOTINFO_FINAL_MARKER: &[u8] = b"UNOS:P1H:BOOTINFO_FINAL";
#[cfg(feature = "exit-boot-services-test")]
const OWNERSHIP_TRANSFERRED_MARKER: &[u8] = b"UNOS:P1H:OWNERSHIP_TRANSFERRED";
#[cfg(feature = "exit-boot-services-test")]
const PHASE_1H_PASS_MARKER: &[u8] = b"UNOS:P1H:PASS";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootInfoLoadError {
    Gop,
    Alloc,
    Map,
    Convert,
    Reservation,
    Framebuffer,
    BootInfo,
    #[cfg(feature = "kernel-handoff-test")]
    PageTable(PageTableLoadError),
    #[cfg(feature = "kernel-handoff-test")]
    CpuPolicy,
    #[cfg(feature = "kernel-handoff-test")]
    Cr3Stability,
    #[cfg(not(feature = "exit-boot-services-test"))]
    Free,
}

impl BootInfoLoadError {
    pub const fn marker(self) -> &'static [u8] {
        match self {
            Self::Gop => b"UNOS:P1G:FAIL:GOP",
            Self::Alloc => b"UNOS:P1G:FAIL:ALLOC",
            Self::Map => b"UNOS:P1G:FAIL:MAP",
            Self::Convert => b"UNOS:P1G:FAIL:CONVERT",
            Self::Reservation => b"UNOS:P1G:FAIL:RESERVATION",
            Self::Framebuffer => b"UNOS:P1G:FAIL:FRAMEBUFFER",
            Self::BootInfo => b"UNOS:P1G:FAIL:BOOTINFO",
            #[cfg(feature = "kernel-handoff-test")]
            Self::PageTable(error) => error.marker(),
            #[cfg(feature = "kernel-handoff-test")]
            Self::CpuPolicy => b"UNOS:P1J:FAIL:CPU_POLICY",
            #[cfg(feature = "kernel-handoff-test")]
            Self::Cr3Stability => b"UNOS:P1J:FAIL:CR3_STABILITY",
            #[cfg(not(feature = "exit-boot-services-test"))]
            Self::Free => b"UNOS:P1G:FAIL:FREE",
        }
    }
}

#[cfg(not(feature = "exit-boot-services-test"))]
pub fn prepare_validate_and_release<B: PageBackend>(
    loaded: &LoadedKernel<B>,
    serial: &mut SerialPort,
) -> Result<(), BootInfoLoadError> {
    let (mut prepared, _) = prepare(loaded, serial, None, &[])?;
    prepared
        .try_release()
        .map_err(|_| BootInfoLoadError::Free)?;
    if !prepared.is_released() {
        return Err(BootInfoLoadError::Free);
    }
    serial.write_line(MEMORY_RELEASED_MARKER);
    serial.write_line(PASS_MARKER);
    Ok(())
}

fn prepare<B: PageBackend>(
    loaded: &LoadedKernel<B>,
    serial: &mut SerialPort,
    stack: Option<PageAllocation>,
    page_table_frames: &[PhysicalFrame],
) -> Result<
    (
        PreparedBootInfo<UefiBootBackend>,
        Option<PageTableReservationProof>,
    ),
    BootInfoLoadError,
> {
    let mut pending = PendingBuffers::allocate().map_err(|_| BootInfoLoadError::Alloc)?;

    let handle =
        boot::get_handle_for_protocol::<GraphicsOutput>().map_err(|_| BootInfoLoadError::Gop)?;
    let mut gop = boot::open_protocol_exclusive::<GraphicsOutput>(handle)
        .map_err(|_| BootInfoLoadError::Gop)?;
    let mode = gop.current_mode_info();
    let (width, height) = mode.resolution();
    let stride = mode.stride();
    let pixel_format = match mode.pixel_format() {
        PixelFormat::Rgb => GopPixelFormat::Rgb,
        PixelFormat::Bgr => GopPixelFormat::Bgr,
        PixelFormat::BltOnly => GopPixelFormat::BltOnly,
        PixelFormat::Bitmask => {
            let masks = mode.pixel_bitmask().ok_or(BootInfoLoadError::Framebuffer)?;
            GopPixelFormat::Bitmask {
                red: masks.red,
                green: masks.green,
                blue: masks.blue,
                reserved: masks.reserved,
            }
        }
    };
    let mut frame_buffer = if mode.pixel_format() == PixelFormat::BltOnly {
        return Err(BootInfoLoadError::Framebuffer);
    } else {
        gop.frame_buffer()
    };
    let framebuffer = convert_framebuffer(GopFramebuffer {
        physical_address: frame_buffer.as_mut_ptr() as u64,
        byte_length: u64::try_from(frame_buffer.size())
            .map_err(|_| BootInfoLoadError::Framebuffer)?,
        width: u32::try_from(width).map_err(|_| BootInfoLoadError::Framebuffer)?,
        height: u32::try_from(height).map_err(|_| BootInfoLoadError::Framebuffer)?,
        pixels_per_scanline: u32::try_from(stride).map_err(|_| BootInfoLoadError::Framebuffer)?,
        pixel_format,
    })
    .map_err(|_| BootInfoLoadError::Framebuffer)?;
    serial.write_line(GOP_READY_MARKER);

    let allocations = pending.allocations().ok_or(BootInfoLoadError::Alloc)?;
    let mut reservations = reservations_for(
        loaded,
        allocations,
        framebuffer.physical_address,
        framebuffer.byte_length,
        stack,
    )
    .map_err(|_| BootInfoLoadError::Reservation)?;
    if !page_table_frames.is_empty() {
        append_page_table_reservations(page_table_frames, &mut reservations)
            .map_err(|_| BootInfoLoadError::Reservation)?;
    }
    reservations
        .finish()
        .map_err(|_| BootInfoLoadError::Reservation)?;
    serial.write_line(BUFFERS_READY_MARKER);

    let map_meta = {
        let raw = pending.raw_map_mut().ok_or(BootInfoLoadError::Alloc)?;
        capture_memory_map(raw).map_err(|_| BootInfoLoadError::Map)?
    };
    serial.write_line(MAP_CAPTURED_MARKER);

    let raw_count = map_meta.map_size / map_meta.descriptor_stride;
    if raw_count == 0 || raw_count > CONVERTED_DESCRIPTOR_CAPACITY {
        return Err(BootInfoLoadError::Convert);
    }
    let (raw, scratch, converted) = pending
        .conversion_buffers_mut()
        .ok_or(BootInfoLoadError::Alloc)?;
    for (index, slot) in scratch[..raw_count].iter_mut().enumerate() {
        let descriptor = read_raw_descriptor(raw, index, map_meta.descriptor_stride)
            .map_err(|_| BootInfoLoadError::Convert)?;
        *slot = MemoryDescriptor {
            kind: map_uefi_memory_kind(descriptor.ty.0),
            reserved0: 0,
            physical_start: descriptor.phys_start,
            page_count: descriptor.page_count,
            attributes: descriptor.att.bits(),
        };
    }
    let normalized_count =
        normalize_mapped_memory_map(scratch, raw_count).map_err(|_| BootInfoLoadError::Convert)?;
    let descriptor_count =
        apply_reservations(&scratch[..normalized_count], &reservations, converted)
            .map_err(|_| BootInfoLoadError::Reservation)?;
    serial.write_line(MAP_CONVERTED_MARKER);

    if !reservation_evidence(&converted[..descriptor_count]) {
        return Err(BootInfoLoadError::Reservation);
    }
    let page_table_proof = if page_table_frames.is_empty() {
        None
    } else {
        Some(
            bootloader::verify_page_table_reservations(
                page_table_frames,
                &converted[..descriptor_count],
            )
            .map_err(|_| BootInfoLoadError::Reservation)?,
        )
    };
    serial.write_line(RESERVATIONS_VALID_MARKER);

    let descriptor_version =
        u16::try_from(map_meta.descriptor_version).map_err(|_| BootInfoLoadError::BootInfo)?;
    let boot_info = build_boot_info(
        allocations.converted_map.page_start,
        &converted[..descriptor_count],
        descriptor_version,
        framebuffer,
    )
    .map_err(|_| BootInfoLoadError::BootInfo)?;
    boot_info
        .validate()
        .map_err(|_| BootInfoLoadError::BootInfo)?;
    pending
        .write_boot_info(boot_info)
        .map_err(|_| BootInfoLoadError::BootInfo)?;
    serial.write_line(BOOTINFO_VALID_MARKER);

    let (backend, allocations) = pending.into_owned_parts().ok_or(BootInfoLoadError::Alloc)?;
    // SAFETY: the converted-map allocation remains owned by `backend`; the
    // count was bounded by its capacity and this temporary view is read-only.
    let descriptors = unsafe {
        core::slice::from_raw_parts(
            allocations.converted_map.page_start as *const MemoryDescriptor,
            descriptor_count,
        )
    };
    let prepared = PreparedBootInfo::from_validated(
        backend,
        allocations,
        boot_info,
        descriptors,
        ProvisionalMapMetadata {
            map_size: u64::try_from(map_meta.map_size).map_err(|_| BootInfoLoadError::BootInfo)?,
            descriptor_stride: u64::try_from(map_meta.descriptor_stride)
                .map_err(|_| BootInfoLoadError::BootInfo)?,
            descriptor_version: map_meta.descriptor_version,
            map_key: u64::try_from(map_meta.map_key).map_err(|_| BootInfoLoadError::BootInfo)?,
            status: MapKeyStatus::Provisional,
        },
    )
    .map_err(|_| BootInfoLoadError::BootInfo)?;
    if prepared.descriptor_count() == 0
        || prepared.descriptor_stride() != MEMORY_DESCRIPTOR_SIZE
        || prepared.wire_size() != BOOT_INFO_SIZE
        || prepared.is_released()
    {
        return Err(BootInfoLoadError::BootInfo);
    }
    serial.write_line(OWNERSHIP_READY_MARKER);
    Ok((prepared, page_table_proof))
}

#[cfg(feature = "exit-boot-services-test")]
pub fn prepare_and_exit<B: PageBackend>(
    #[cfg_attr(not(feature = "cpu-readiness-failure-test"), allow(unused_mut))]
    mut loaded: LoadedKernel<B>,
    serial: &mut SerialPort,
) -> Result<Infallible, BootInfoLoadError> {
    #[cfg(feature = "kernel-handoff-test")]
    #[cfg_attr(not(feature = "cpu-readiness-failure-test"), allow(unused_mut))]
    let mut stack = allocate_bootstrap_stack(&loaded)?;
    #[cfg(feature = "kernel-handoff-test")]
    let observed_cpu = crate::cpu_probe::capture();
    #[cfg(feature = "kernel-handoff-test")]
    serial.write_line(crate::cpu_probe::STATE_CAPTURED_MARKER);
    #[cfg(feature = "kernel-handoff-test")]
    #[cfg_attr(not(feature = "cpu-readiness-failure-test"), allow(unused_mut))]
    let mut verified_page_tables = page_table_loader::construct_inactive(&loaded, &stack, serial)
        .map_err(BootInfoLoadError::PageTable)?;
    #[cfg(feature = "kernel-handoff-test")]
    let policy_cpu = observed_cpu;
    #[cfg(feature = "cpu-readiness-failure-test")]
    let policy_cpu = {
        let mut policy_cpu = policy_cpu;
        policy_cpu.extended_feature_edx &= !(1 << 20);
        policy_cpu
    };
    #[cfg(feature = "kernel-handoff-test")]
    let validated_cpu = match policy_cpu.validate() {
        Ok(value) => value,
        Err(_) => {
            #[cfg(feature = "cpu-readiness-failure-test")]
            {
                let page_tables_released = verified_page_tables.try_release().is_ok()
                    && verified_page_tables.frame_count() == 0;
                let stack_released = stack.try_release().is_ok() && stack.is_released();
                let kernel_released = loaded.try_release().is_ok() && loaded.is_released();
                if page_tables_released && stack_released && kernel_released {
                    serial.write_line(crate::cpu_probe::ROLLBACK_MARKER);
                }
            }
            return Err(BootInfoLoadError::CpuPolicy);
        }
    };
    #[cfg(feature = "kernel-handoff-test")]
    serial.write_line(crate::cpu_probe::CAPABILITIES_VALIDATED_MARKER);
    #[cfg(feature = "kernel-handoff-test")]
    let mapped_end = (loaded.entry_point() & !(bootloader::UEFI_PAGE_SIZE - 1))
        .checked_add(bootloader::UEFI_PAGE_SIZE)
        .ok_or(BootInfoLoadError::CpuPolicy)?
        .max(stack.top());
    #[cfg(feature = "kernel-handoff-test")]
    let readiness = validated_cpu
        .classify_for_hierarchy(
            verified_page_tables.frames(),
            verified_page_tables.root_frame(),
            mapped_end,
        )
        .map_err(|_| BootInfoLoadError::CpuPolicy)?;
    #[cfg(feature = "kernel-handoff-test")]
    serial.write_line(crate::cpu_probe::REQUIREMENTS_CLASSIFIED_MARKER);
    #[cfg(feature = "kernel-handoff-test")]
    readiness
        .cr3_stability_token()
        .verify(crate::cpu_probe::read_cr3())
        .map_err(|_| BootInfoLoadError::Cr3Stability)?;
    #[cfg(feature = "kernel-handoff-test")]
    serial.write_line(crate::cpu_probe::HIERARCHY_COMPATIBLE_MARKER);
    #[cfg(feature = "kernel-handoff-test")]
    let activation_prepared = verified_page_tables
        .prepare_activation(readiness)
        .map_err(|_| BootInfoLoadError::CpuPolicy)?;
    #[cfg(not(feature = "kernel-handoff-test"))]
    let (prepared, _) = prepare(&loaded, serial, None, &[])?;
    #[cfg(feature = "kernel-handoff-test")]
    let (prepared, reservation_proof) = prepare(
        &loaded,
        serial,
        Some(stack.allocation()),
        activation_prepared.frames(),
    )?;
    #[cfg(feature = "kernel-handoff-test")]
    let page_tables = activation_prepared
        .confirm_final_map_reservation(reservation_proof.ok_or(BootInfoLoadError::Reservation)?)
        .map_err(|_| BootInfoLoadError::PageTable(PageTableLoadError::Reserve))?;
    #[cfg(feature = "kernel-handoff-test")]
    page_tables
        .readiness()
        .cr3_stability_token()
        .verify(crate::cpu_probe::read_cr3())
        .map_err(|_| BootInfoLoadError::Cr3Stability)?;
    let ready = BootServicesState::new()
        .with_kernel(loaded)
        .and_then(|state| state.with_boot_information(prepared))
        .map_err(|_| BootInfoLoadError::BootInfo)?;

    #[cfg(feature = "kernel-handoff-test")]
    let ready = ready
        .with_bootstrap_stack_and_page_tables(stack, page_tables)
        .map_err(|_| BootInfoLoadError::BootInfo)?;

    boot::set_watchdog_timer(0, 0, None).map_err(|_| BootInfoLoadError::BootInfo)?;
    let allocations = ready.boot_allocations();
    let raw_length = allocation_byte_length(allocations.raw_map).ok_or(BootInfoLoadError::Map)?;
    // SAFETY: ExitReady owns the complete raw-map page allocation. This is the
    // sole mutable view and its checked length exactly matches that allocation.
    let raw = unsafe {
        core::slice::from_raw_parts_mut(allocations.raw_map.page_start as *mut u8, raw_length)
    };

    let system_table = uefi::table::system_table_raw().ok_or(BootInfoLoadError::Map)?;
    // SAFETY: the entry macro installed a live system table and boot services
    // have not been exited. Copying the function pointer creates no guard.
    let exit_boot_services = unsafe {
        system_table
            .as_ref()
            .boot_services
            .as_ref()
            .ok_or(BootInfoLoadError::Map)?
            .exit_boot_services
    };
    let image_handle = boot::image_handle().as_ptr();
    let mut final_map = capture_memory_map(raw).map_err(|_| BootInfoLoadError::Map)?;

    serial.write_line(EXIT_READY_MARKER);
    ready.cross_exit_boundary(|transferred| {
        let mut retry = ExitRetryPolicy::new();
        if !retry.begin_attempt() {
            post_exit_failure(serial, b"UNOS:P1H:FAIL:EXIT");
        }
        // SAFETY: the final key was captured after all allocations, protocol
        // guards and watchdog work. No firmware call occurred in between.
        let mut status = unsafe { exit_boot_services(image_handle, final_map.map_key) };
        if retry.retry_invalid_key(status == Status::INVALID_PARAMETER) {
            match capture_memory_map(raw) {
                Ok(retry_map) => {
                    final_map = retry_map;
                    if !retry.begin_attempt() {
                        post_exit_failure(serial, b"UNOS:P1H:FAIL:EXIT");
                    }
                    // SAFETY: this is the one allowed retry, immediately after
                    // obtaining a fresh key and without an intervening call.
                    status = unsafe { exit_boot_services(image_handle, final_map.map_key) };
                }
                Err(_) => post_exit_failure(serial, b"UNOS:P1H:FAIL:FINAL_MAP"),
            }
        }
        if status != Status::SUCCESS {
            post_exit_failure(serial, b"UNOS:P1H:FAIL:EXIT");
        }
        serial.write_line(BOOT_SERVICES_EXITED_MARKER);
        finalize_post_exit(transferred, final_map, raw, serial)
    })
}

#[cfg(feature = "exit-boot-services-test")]
fn finalize_post_exit(
    transferred: TransferredBootState,
    final_map: CapturedMapMeta,
    raw: &[u8],
    serial: &mut SerialPort,
) -> Infallible {
    let stack = transferred.bootstrap_stack();
    let allocations = transferred.boot_allocations();
    let scratch_length = match allocation_byte_length(allocations.conversion_scratch) {
        Some(length) => length / size_of::<MemoryDescriptor>(),
        None => post_exit_failure(serial, b"UNOS:P1H:FAIL:FINAL_MAP"),
    };
    let converted_length = match allocation_byte_length(allocations.converted_map) {
        Some(length) => length / size_of::<MemoryDescriptor>(),
        None => post_exit_failure(serial, b"UNOS:P1H:FAIL:FINAL_MAP"),
    };
    if scratch_length < CONVERTED_DESCRIPTOR_CAPACITY
        || converted_length < CONVERTED_DESCRIPTOR_CAPACITY
    {
        post_exit_failure(serial, b"UNOS:P1H:FAIL:FINAL_MAP");
    }
    // SAFETY: ownership was transferred from four distinct live allocations.
    // The post-exit state is their sole owner and both slice lengths are bounded
    // by the corresponding allocation metadata.
    let (scratch, converted) = unsafe {
        (
            core::slice::from_raw_parts_mut(
                allocations.conversion_scratch.page_start as *mut MemoryDescriptor,
                CONVERTED_DESCRIPTOR_CAPACITY,
            ),
            core::slice::from_raw_parts_mut(
                allocations.converted_map.page_start as *mut MemoryDescriptor,
                CONVERTED_DESCRIPTOR_CAPACITY,
            ),
        )
    };

    let raw_count = final_map.map_size / final_map.descriptor_stride;
    if raw_count == 0 || raw_count > CONVERTED_DESCRIPTOR_CAPACITY {
        post_exit_failure(serial, b"UNOS:P1H:FAIL:FINAL_MAP");
    }
    for (index, slot) in scratch[..raw_count].iter_mut().enumerate() {
        let descriptor = match read_raw_descriptor(raw, index, final_map.descriptor_stride) {
            Ok(descriptor) => descriptor,
            Err(()) => post_exit_failure(serial, b"UNOS:P1H:FAIL:FINAL_MAP"),
        };
        *slot = MemoryDescriptor {
            kind: map_uefi_memory_kind_post_exit(descriptor.ty.0),
            reserved0: 0,
            physical_start: descriptor.phys_start,
            page_count: descriptor.page_count,
            attributes: descriptor.att.bits(),
        };
    }
    let normalized_count = match normalize_mapped_memory_map(scratch, raw_count) {
        Ok(count) => count,
        Err(_) => post_exit_failure(serial, b"UNOS:P1H:FAIL:FINAL_MAP"),
    };
    let mut reservations = match reservations_for_transferred(&transferred, stack) {
        Ok(reservations) => reservations,
        Err(_) => post_exit_failure(serial, b"UNOS:P1H:FAIL:FINAL_MAP"),
    };
    if reservations.finish().is_err() {
        post_exit_failure(serial, b"UNOS:P1H:FAIL:FINAL_MAP");
    }
    let descriptor_count =
        match apply_reservations(&scratch[..normalized_count], &reservations, converted) {
            Ok(count) => count,
            Err(_) => post_exit_failure(serial, b"UNOS:P1H:FAIL:FINAL_MAP"),
        };
    if !reservation_evidence(&converted[..descriptor_count]) {
        post_exit_failure(serial, b"UNOS:P1H:FAIL:FINAL_MAP");
    }
    serial.write_line(FINAL_MAP_CONVERTED_MARKER);
    #[cfg(feature = "kernel-handoff-test")]
    let page_tables = match transferred.page_tables() {
        Some(page_tables) => page_tables,
        None => post_exit_failure(serial, b"UNOS:P1J:FAIL:RESERVATION"),
    };
    #[cfg(feature = "kernel-handoff-test")]
    if bootloader::verify_page_table_reservations(
        page_tables.frames(),
        &converted[..descriptor_count],
    )
    .is_err()
    {
        post_exit_failure(serial, b"UNOS:P1J:FAIL:RESERVATION");
    }
    #[cfg(feature = "kernel-handoff-test")]
    let page_table_root = page_tables.root_frame().address();
    #[cfg(feature = "kernel-handoff-test")]
    let page_table_count = page_tables.frame_count();
    #[cfg(feature = "kernel-handoff-test")]
    serial.write_line(page_table_loader::FINAL_MAP_RESERVED_MARKER);

    let descriptor_version = match u16::try_from(final_map.descriptor_version) {
        Ok(version) => version,
        Err(_) => post_exit_failure(serial, b"UNOS:P1H:FAIL:BOOTINFO"),
    };
    let boot_info = match build_boot_info(
        allocations.converted_map.page_start,
        &converted[..descriptor_count],
        descriptor_version,
        transferred.framebuffer(),
    ) {
        Ok(boot_info) => boot_info,
        Err(_) => post_exit_failure(serial, b"UNOS:P1H:FAIL:BOOTINFO"),
    };
    // SAFETY: the dedicated transferred page is aligned, still live, and large
    // enough for the unchanged 128-byte repr(C) wire value.
    unsafe { (transferred.boot_info_address() as *mut boot_protocol::BootInfo).write(boot_info) };
    let final_metadata = FinalMapMetadata {
        map_size: match u64::try_from(final_map.map_size) {
            Ok(value) => value,
            Err(_) => post_exit_failure(serial, b"UNOS:P1H:FAIL:BOOTINFO"),
        },
        descriptor_stride: match u64::try_from(final_map.descriptor_stride) {
            Ok(value) => value,
            Err(_) => post_exit_failure(serial, b"UNOS:P1H:FAIL:BOOTINFO"),
        },
        descriptor_version: final_map.descriptor_version,
        map_key: match u64::try_from(final_map.map_key) {
            Ok(value) => value,
            Err(_) => post_exit_failure(serial, b"UNOS:P1H:FAIL:BOOTINFO"),
        },
    };
    let post_exit = match PostExitState::from_final_map(
        transferred,
        boot_info,
        &converted[..descriptor_count],
        final_metadata,
    ) {
        Ok(state) => state,
        Err(_) => post_exit_failure(serial, b"UNOS:P1H:FAIL:BOOTINFO"),
    };
    #[cfg(feature = "kernel-handoff-test")]
    let activation_readiness = match post_exit.activation_readiness() {
        Some(value) => value,
        None => post_exit_failure(serial, b"UNOS:P1J:FAIL:CPU_POLICY"),
    };
    #[cfg(feature = "kernel-handoff-test")]
    if activation_readiness
        .cr3_stability_token()
        .verify(crate::cpu_probe::read_cr3())
        .is_err()
    {
        post_exit_failure(serial, b"UNOS:P1J:FAIL:CR3_STABILITY");
    }
    #[cfg(feature = "kernel-handoff-test")]
    serial.write_line(crate::cpu_probe::CR3_UNCHANGED_MARKER);
    #[cfg(feature = "kernel-handoff-test")]
    let page_table_state_valid = post_exit.page_table_root_frame() == Some(page_table_root)
        && post_exit.page_table_frame_count() == page_table_count;
    #[cfg(not(feature = "kernel-handoff-test"))]
    let page_table_state_valid = post_exit.page_table_frame_count() == 0;
    if post_exit.descriptor_count() == 0
        || post_exit.boot_info_address() == 0
        || post_exit.kernel_entry() == 0
        || !page_table_state_valid
    {
        post_exit_failure(serial, b"UNOS:P1H:FAIL:BOOTINFO");
    }
    serial.write_line(BOOTINFO_FINAL_MARKER);
    #[cfg(feature = "kernel-handoff-test")]
    serial.write_line(crate::cpu_probe::ACTIVATION_PREPARED_MARKER);
    serial.write_line(OWNERSHIP_TRANSFERRED_MARKER);
    #[cfg(feature = "kernel-handoff-test")]
    serial.write_line(page_table_loader::OWNERSHIP_TRANSFERRED_MARKER);
    serial.write_line(PHASE_1H_PASS_MARKER);
    #[cfg(feature = "kernel-handoff-test")]
    {
        let stack = match stack {
            Some(stack) => stack,
            None => post_exit_failure(serial, b"UNOS:P1I:FAIL:STACK"),
        };
        // SAFETY: the lower sentinel is in the live transferred stack allocation
        // and is read before switching RSP.
        if unsafe { (stack.canary_address as *const u64).read() } != stack.canary_value {
            post_exit_failure(serial, b"UNOS:P1I:FAIL:STACK");
        }
        crate::handoff::jump_to_kernel(
            post_exit.kernel_entry(),
            post_exit.boot_info_address(),
            stack.bottom,
            stack.top,
            serial,
        )
    }
    #[cfg(not(feature = "kernel-handoff-test"))]
    crate::test_exit::success()
}

#[cfg(feature = "exit-boot-services-test")]
fn post_exit_failure(serial: &mut SerialPort, marker: &[u8]) -> ! {
    serial.write_line(marker);
    crate::test_exit::failure()
}

#[cfg(feature = "exit-boot-services-test")]
fn allocation_byte_length(allocation: PageAllocation) -> Option<usize> {
    usize::try_from(allocation.page_count.checked_mul(4096)?).ok()
}

fn reservation_evidence(descriptors: &[MemoryDescriptor]) -> bool {
    let mut kernel = false;
    let mut boot_info = false;
    let mut map_buffers = 0;
    let mut bootstrap_stack = false;
    let mut page_tables = false;
    for descriptor in descriptors {
        match descriptor.kind {
            MEMORY_KIND_KERNEL_IMAGE => kernel = true,
            MEMORY_KIND_BOOT_INFO => boot_info = true,
            MEMORY_KIND_BOOT_MEMORY_MAP => map_buffers += 1,
            MEMORY_KIND_BOOTSTRAP_STACK => bootstrap_stack = true,
            boot_protocol::MEMORY_KIND_PAGE_TABLE => page_tables = true,
            _ => {}
        }
    }
    kernel
        && boot_info
        && map_buffers >= 3
        && (!cfg!(feature = "kernel-handoff-test") || bootstrap_stack)
        && (!cfg!(feature = "kernel-handoff-test") || page_tables)
}

fn reservations_for<B: PageBackend>(
    loaded: &LoadedKernel<B>,
    allocations: BootDataAllocations,
    framebuffer_start: u64,
    framebuffer_length: u64,
    stack: Option<PageAllocation>,
) -> Result<ReservationList, MapBuildError> {
    let mut reservations = ReservationList::new();
    for segment in loaded.segment_metadata() {
        reservations.push(Reservation {
            physical_start: segment.allocation_start,
            page_count: segment.page_count,
            kind: MEMORY_KIND_KERNEL_IMAGE,
            source: ReservationSource::KernelImage,
        })?;
    }
    for (allocation, source, kind) in [
        (
            allocations.raw_map,
            ReservationSource::RawMemoryMap,
            MEMORY_KIND_BOOT_MEMORY_MAP,
        ),
        (
            allocations.conversion_scratch,
            ReservationSource::ConversionScratch,
            MEMORY_KIND_BOOT_MEMORY_MAP,
        ),
        (
            allocations.converted_map,
            ReservationSource::ConvertedMemoryMap,
            MEMORY_KIND_BOOT_MEMORY_MAP,
        ),
        (
            allocations.boot_info,
            ReservationSource::BootInfo,
            MEMORY_KIND_BOOT_INFO,
        ),
    ] {
        reservations.push(Reservation {
            physical_start: allocation.page_start,
            page_count: allocation.page_count,
            kind,
            source,
        })?;
    }
    let framebuffer_end = framebuffer_start
        .checked_add(framebuffer_length)
        .ok_or(MapBuildError::RangeOverflow)?;
    let page_start = framebuffer_start & !0xfff;
    let page_end = framebuffer_end
        .checked_add(0xfff)
        .ok_or(MapBuildError::RangeOverflow)?
        & !0xfff;
    reservations.push(Reservation {
        physical_start: page_start,
        page_count: (page_end - page_start) / 4096,
        kind: MEMORY_KIND_FRAMEBUFFER,
        source: ReservationSource::Framebuffer,
    })?;
    if let Some(stack) = stack {
        reservations.push(Reservation {
            physical_start: stack.page_start,
            page_count: stack.page_count,
            kind: MEMORY_KIND_BOOTSTRAP_STACK,
            source: ReservationSource::BootstrapStack,
        })?;
    }
    Ok(reservations)
}

#[cfg(feature = "exit-boot-services-test")]
fn reservations_for_transferred(
    transferred: &TransferredBootState,
    stack: Option<TransferredBootstrapStack>,
) -> Result<ReservationList, MapBuildError> {
    let allocations = transferred.boot_allocations();
    let framebuffer = transferred.framebuffer();
    let mut reservations = ReservationList::new();
    for segment in transferred.kernel_segments() {
        reservations.push(Reservation {
            physical_start: segment.allocation_start,
            page_count: segment.page_count,
            kind: MEMORY_KIND_KERNEL_IMAGE,
            source: ReservationSource::KernelImage,
        })?;
    }
    for (allocation, source, kind) in [
        (
            allocations.raw_map,
            ReservationSource::RawMemoryMap,
            MEMORY_KIND_BOOT_MEMORY_MAP,
        ),
        (
            allocations.conversion_scratch,
            ReservationSource::ConversionScratch,
            MEMORY_KIND_BOOT_MEMORY_MAP,
        ),
        (
            allocations.converted_map,
            ReservationSource::ConvertedMemoryMap,
            MEMORY_KIND_BOOT_MEMORY_MAP,
        ),
        (
            allocations.boot_info,
            ReservationSource::BootInfo,
            MEMORY_KIND_BOOT_INFO,
        ),
    ] {
        reservations.push(Reservation {
            physical_start: allocation.page_start,
            page_count: allocation.page_count,
            kind,
            source,
        })?;
    }
    let framebuffer_end = framebuffer
        .physical_address
        .checked_add(framebuffer.byte_length)
        .ok_or(MapBuildError::RangeOverflow)?;
    let page_start = framebuffer.physical_address & !0xfff;
    let page_end = framebuffer_end
        .checked_add(0xfff)
        .ok_or(MapBuildError::RangeOverflow)?
        & !0xfff;
    reservations.push(Reservation {
        physical_start: page_start,
        page_count: (page_end - page_start) / 4096,
        kind: MEMORY_KIND_FRAMEBUFFER,
        source: ReservationSource::Framebuffer,
    })?;
    if let Some(stack) = stack {
        reservations.push(Reservation {
            physical_start: stack.bottom,
            page_count: BOOTSTRAP_STACK_PAGES,
            kind: MEMORY_KIND_BOOTSTRAP_STACK,
            source: ReservationSource::BootstrapStack,
        })?;
    }
    if let Some(page_tables) = transferred.page_tables() {
        append_page_table_reservations(page_tables.frames(), &mut reservations)
            .map_err(|_| MapBuildError::ReservationCapacity)?;
    }
    Ok(reservations)
}

#[cfg(feature = "kernel-handoff-test")]
fn allocate_bootstrap_stack<B: PageBackend>(
    loaded: &LoadedKernel<B>,
) -> Result<BootstrapStack<UefiBootBackend>, BootInfoLoadError> {
    let pointer = boot::allocate_pages(
        AllocateType::MaxAddress(BOOTSTRAP_IDENTITY_LIMIT - 1),
        MemoryType::LOADER_DATA,
        usize::try_from(BOOTSTRAP_STACK_PAGES).map_err(|_| BootInfoLoadError::Alloc)?,
    )
    .map_err(|_| BootInfoLoadError::Alloc)?;
    let allocation = PageAllocation {
        page_start: pointer.as_ptr() as u64,
        page_count: BOOTSTRAP_STACK_PAGES,
    };
    let length =
        usize::try_from(BOOTSTRAP_STACK_PAGES * 4096).map_err(|_| BootInfoLoadError::Alloc)?;
    // SAFETY: this is the complete uniquely-owned fresh allocation. Every byte
    // is initialized before the stack owner is constructed.
    unsafe { core::slice::from_raw_parts_mut(pointer.as_ptr(), length) }.fill(0);
    // SAFETY: the allocation is page aligned and initialized; the sentinel is
    // the first aligned word and is never used as normal stack storage.
    unsafe { (allocation.page_start as *mut u64).write(BOOTSTRAP_STACK_CANARY) };
    let mut forbidden = [PhysicalRange { start: 0, end: 0 }; bootloader::MAX_LOAD_ITEMS];
    let mut count = 0;
    for segment in loaded.segment_metadata() {
        forbidden[count] = PhysicalRange {
            start: segment.allocation_start,
            end: segment.allocation_end,
        };
        count += 1;
    }
    BootstrapStack::from_initialized(UefiBootBackend, allocation, &forbidden[..count])
        .map_err(|_| BootInfoLoadError::Alloc)
}

#[derive(Clone, Copy)]
struct CapturedMapMeta {
    map_size: usize,
    descriptor_stride: usize,
    descriptor_version: u32,
    map_key: usize,
}

fn capture_memory_map(buffer: &mut [u8]) -> Result<CapturedMapMeta, Status> {
    for attempt in 0..=bootloader::RAW_MEMORY_MAP_RETRY_LIMIT {
        let mut map_size = buffer.len();
        let mut map_key = 0_usize;
        let mut descriptor_stride = 0_usize;
        let mut descriptor_version = 0_u32;
        let system_table = uefi::table::system_table_raw().ok_or(Status::NOT_READY)?;
        // SAFETY: the UEFI entry macro installed the live system-table pointer;
        // boot services are active and the page-aligned buffer is writable for
        // `map_size` bytes. The returned stride is validated before parsing.
        let status = unsafe {
            let boot_services = system_table
                .as_ref()
                .boot_services
                .as_ref()
                .ok_or(Status::NOT_READY)?;
            (boot_services.get_memory_map)(
                &mut map_size,
                buffer.as_mut_ptr().cast::<UefiMemoryDescriptor>(),
                &mut map_key,
                &mut descriptor_stride,
                &mut descriptor_version,
            )
        };
        if status == Status::SUCCESS {
            if descriptor_stride < size_of::<UefiMemoryDescriptor>()
                || map_size == 0
                || map_size > buffer.len()
                || !map_size.is_multiple_of(descriptor_stride)
            {
                return Err(Status::COMPROMISED_DATA);
            }
            return Ok(CapturedMapMeta {
                map_size,
                descriptor_stride,
                descriptor_version,
                map_key,
            });
        }
        if status != Status::BUFFER_TOO_SMALL
            || map_size > buffer.len()
            || !retry_is_allowed(attempt)
        {
            return Err(status);
        }
    }
    Err(Status::ABORTED)
}

fn read_raw_descriptor(
    buffer: &[u8],
    index: usize,
    stride: usize,
) -> Result<UefiMemoryDescriptor, ()> {
    if stride < size_of::<UefiMemoryDescriptor>() {
        return Err(());
    }
    let offset = index.checked_mul(stride).ok_or(())?;
    let end = offset
        .checked_add(size_of::<UefiMemoryDescriptor>())
        .ok_or(())?;
    if end > buffer.len() {
        return Err(());
    }
    // SAFETY: bounds were checked, the allocation is page-aligned, UEFI
    // guarantees the standard descriptor prefix at each reported stride, and
    // the value is copied without retaining a reference into the raw map.
    Ok(unsafe {
        buffer
            .as_ptr()
            .add(offset)
            .cast::<UefiMemoryDescriptor>()
            .read()
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BufferError {
    Allocate,
    Free,
    Range,
}

struct UefiBootBackend;

impl PageBackend for UefiBootBackend {
    type Error = BufferError;
    fn allocate_at(&mut self, page_start: u64, page_count: u64) -> Result<(), Self::Error> {
        let pages = usize::try_from(page_count).map_err(|_| BufferError::Range)?;
        let pointer = boot::allocate_pages(
            AllocateType::Address(page_start),
            MemoryType::LOADER_DATA,
            pages,
        )
        .map_err(|_| BufferError::Allocate)?;
        if pointer.as_ptr() as u64 == page_start {
            Ok(())
        } else {
            Err(BufferError::Allocate)
        }
    }
    fn free(&mut self, page_start: u64, page_count: u64) -> Result<(), Self::Error> {
        let pages = usize::try_from(page_count).map_err(|_| BufferError::Range)?;
        let pointer = NonNull::new(page_start as *mut u8).ok_or(BufferError::Range)?;
        // SAFETY: ownership metadata records the exact allocation and marks it
        // released only after this firmware call succeeds.
        unsafe { boot::free_pages(pointer, pages) }.map_err(|_| BufferError::Free)
    }
}

struct PendingBuffers {
    backend: Option<UefiBootBackend>,
    allocations: [Option<PageAllocation>; 4],
}

impl PendingBuffers {
    fn allocate() -> Result<Self, BufferError> {
        let mut pending = Self {
            backend: Some(UefiBootBackend),
            allocations: [None; 4],
        };
        for (index, pages) in [
            RAW_MAP_PAGES,
            CONVERTED_MAP_PAGES,
            CONVERTED_MAP_PAGES,
            BOOT_INFO_PAGES,
        ]
        .into_iter()
        .enumerate()
        {
            let pointer =
                boot::allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, pages)
                    .map_err(|_| BufferError::Allocate)?;
            let allocation = PageAllocation {
                page_start: pointer.as_ptr() as u64,
                page_count: u64::try_from(pages).map_err(|_| BufferError::Range)?,
            };
            pending.allocations[index] = Some(allocation);
            // SAFETY: this is the complete, uniquely owned allocation returned
            // above. It is initialized before any typed view or map capture.
            unsafe { core::slice::from_raw_parts_mut(pointer.as_ptr(), pages * 4096) }.fill(0);
        }
        Ok(pending)
    }

    fn allocations(&self) -> Option<BootDataAllocations> {
        Some(BootDataAllocations {
            raw_map: self.allocations[0]?,
            conversion_scratch: self.allocations[1]?,
            converted_map: self.allocations[2]?,
            boot_info: self.allocations[3]?,
        })
    }

    fn allocation_bytes_mut(&mut self, index: usize) -> Option<&mut [u8]> {
        let allocation = self.allocations[index]?;
        let length = usize::try_from(allocation.page_count.checked_mul(4096)?).ok()?;
        // SAFETY: PendingBuffers uniquely owns the page allocation at `index`;
        // returned slices are scoped to `&mut self` and allocations never overlap.
        Some(unsafe { core::slice::from_raw_parts_mut(allocation.page_start as *mut u8, length) })
    }

    fn raw_map_mut(&mut self) -> Option<&mut [u8]> {
        self.allocation_bytes_mut(0)
    }
    fn conversion_buffers_mut(
        &mut self,
    ) -> Option<(&[u8], &mut [MemoryDescriptor], &mut [MemoryDescriptor])> {
        let raw = self.allocations[0]?;
        let scratch = self.allocations[1]?;
        let converted = self.allocations[2]?;
        let raw_length = usize::try_from(raw.page_count.checked_mul(4096)?).ok()?;
        // SAFETY: all three values came from distinct live AllocateAnyPages
        // calls. PendingBuffers exclusively owns them and each length matches
        // its page-aligned allocation.
        Some(unsafe {
            (
                core::slice::from_raw_parts(raw.page_start as *const u8, raw_length),
                core::slice::from_raw_parts_mut(
                    scratch.page_start as *mut MemoryDescriptor,
                    CONVERTED_DESCRIPTOR_CAPACITY,
                ),
                core::slice::from_raw_parts_mut(
                    converted.page_start as *mut MemoryDescriptor,
                    CONVERTED_DESCRIPTOR_CAPACITY,
                ),
            )
        })
    }
    fn write_boot_info(&mut self, boot_info: boot_protocol::BootInfo) -> Result<(), BufferError> {
        let allocation = self.allocations[3].ok_or(BufferError::Range)?;
        // SAFETY: the dedicated page is aligned, uniquely owned and large
        // enough for the 128-byte repr(C) BootInfo value.
        unsafe { (allocation.page_start as *mut boot_protocol::BootInfo).write(boot_info) };
        Ok(())
    }
    fn into_owned_parts(mut self) -> Option<(UefiBootBackend, BootDataAllocations)> {
        let allocations = self.allocations()?;
        self.allocations.fill(None);
        Some((self.backend.take()?, allocations))
    }
    fn try_release(&mut self) -> Result<(), BufferError> {
        let backend = self.backend.as_mut().ok_or(BufferError::Free)?;
        for index in (0..self.allocations.len()).rev() {
            if let Some(allocation) = self.allocations[index] {
                backend.free(allocation.page_start, allocation.page_count)?;
                self.allocations[index] = None;
            }
        }
        Ok(())
    }
}

impl Drop for PendingBuffers {
    fn drop(&mut self) {
        if self.try_release().is_err()
            && let Some(backend) = self.backend.as_mut()
        {
            for index in (0..self.allocations.len()).rev() {
                if let Some(allocation) = self.allocations[index]
                    && backend
                        .free(allocation.page_start, allocation.page_count)
                        .is_ok()
                {
                    self.allocations[index] = None;
                }
            }
        }
    }
}
