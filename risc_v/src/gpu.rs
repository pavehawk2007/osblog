// gpu.rs
// Graphics stuff
// Stephen Marz
// 12 May 2020

#![allow(dead_code)]
use crate::{page::{zalloc, PAGE_SIZE},
			kmem::{kmalloc, kfree},
            virtio,
            virtio::{MmioOffsets, Queue, StatusField, VIRTIO_RING_SIZE, Descriptor, VIRTIO_DESC_F_WRITE, VIRTIO_DESC_F_NEXT}};
use core::{mem::size_of, ptr::null_mut};
// use alloc::boxed::Box;

pub const F_VIRGL: u32 = 0;
pub const F_EDID: u32 = 1;

pub const EVENT_DISPLAY: u32 = 1 << 0;
#[repr(C)]
pub struct Config {
	//events_read signals pending events to the driver. The driver MUST NOT write to this field.
	// events_clear clears pending events in the device. Writing a ’1’ into a bit will clear the corresponding bit in events_read mimicking write-to-clear behavior.
	//num_scanouts specifies the maximum number of scanouts supported by the device. Minimum value is 1, maximum value is 16.
	events_read: u32,
	events_clear: u32,
	num_scanouts: u32,
	reserved: u32,
}
#[repr(u32)]
pub enum CtrlType {
	/* 2d commands */
	CmdGetDisplayInfo = 0x0100,
	CmdResourceCreate2d,
	CmdResourceUref,
	CmdSetScanout,
	CmdResourceFlush,
	CmdTransferToHost2d,
	CmdResourceAttachBacking,
	CmdResourceDetachBacking,
	CmdGetCapsetInfo,
	CmdGetCapset,
	CmdGetEdid,
	/* cursor commands */
	CmdUpdateCursor = 0x0300,
	CmdMoveCursor,
	/* success responses */
	RespOkNoData = 0x1100,
	RespOkDisplayInfo,
	RespOkCapsetInfo,
	RespOkCapset,
	RespOkEdid,
	/* error responses */
	RespErrUnspec = 0x1200,
	RespErrOutOfMemory,
	RespErrInvalidScanoutId,
	RespErrInvalidResourceId,
	RespErrInvalidContextId,
	RespErrInvalidParameter,
}

pub const FLAG_FENCE: u32= 1 << 0;
#[repr(C)]
pub struct CtrlHeader {
	ctrl_type: CtrlType,
	flags: u32,
	fence_id: u64,
	ctx_id: u32,
	padding: u32
}

pub const MAX_SCANOUTS: usize = 16;
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rect {
	x: u32,
	y: u32,
	width: u32,
	height: u32,
}

impl Rect {
	pub fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
		Self {
			x, y, width, height
		}
	}
}
#[repr(C)]
pub struct DisplayOne {
	r: Rect,
	enabled: u32,
	flags: u32,
}

#[repr(C)]
pub struct RespDisplayInfo {
	hdr: CtrlHeader,
	pmodes: [DisplayOne; MAX_SCANOUTS],
}
#[repr(C)]
pub struct GetEdid {
	hdr: CtrlHeader,
	scanout: u32,
	padding: u32,
}
#[repr(C)]
pub struct RespEdid {
	hdr: CtrlHeader,
	size: u32,
	padding: u32,
	edid: [u8; 1024],
}
#[repr(u32)]
pub enum Formats {
	B8G8R8A8Unorm = 1,
	B8G8R8X8Unorm = 2,
	A8R8G8B8Unorm = 3,
	X8R8G8B8Unorm = 4,
	R8G8B8A8Unorm = 67,
	X8B8G8R8Unorm = 68,
	A8B8G8R8Unorm = 121,
	R8G8B8X8Unorm = 134,
}

#[repr(C)]
pub struct ResourceCreate2d {
	hdr: CtrlHeader,
	resource_id: u32,
	format: Formats,
	width: u32,
	height: u32,
}
#[repr(C)]
pub struct ResourceUnref {
	hdr: CtrlHeader,
	resource_id: u32,
	padding: u32,
}
#[repr(C)]
pub struct SetScanout {
	hdr: CtrlHeader,
	r: Rect,
	scanout_id: u32,
	resource_id: u32,
}
#[repr(C)]
pub struct ResourceFlush {
	hdr: CtrlHeader,
	r: Rect,
	resource_id: u32,
	padding: u32,
}

#[repr(C)]
pub struct TransferToHost2d {
	hdr: CtrlHeader,
	r: Rect,
	offset: u64,
	resource_id: u32,
	padding: u32,
}
#[repr(C)]
pub struct AttachBacking {
	hdr: CtrlHeader,
	resource_id: u32,
	nr_entries: u32,
}

#[repr(C)]
pub struct MemEntry {
	addr: u64,
	length: u32,
	padding: u32,
}

#[repr(C)]
pub struct DetachBacking {
	hdr: CtrlHeader,
	resource_id: u32,
	padding: u32,
}
#[repr(C)]
pub struct CursorPos {
	scanout_id: u32,
	x: u32,
	y: u32,
	padding: u32,
}

#[repr(C)]
pub struct UpdateCursor {
	hdr: CtrlHeader,
	pos: CursorPos,
	resource_id: u32,
	hot_x: u32,
	hot_y: u32,
	padding: u32,
}

#[derive(Clone, Copy)]
pub struct Pixel {
	r: u8,
	g: u8,
	b: u8,
	a: u8,
}
impl Pixel {
	pub fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
		Self {
			r, g, b, a
		}
	}
}

// This is not in the specification, but this makes
// it easier for us to do just a single kfree.
pub struct Request<RqT, RpT> {
	request: RqT,
	response: RpT,
}

impl<RqT, RpT> Request<RqT, RpT> {
	pub fn new(request: RqT) -> *mut Self {
		let sz = size_of::<RqT>() + size_of::<RpT>();
		let ptr = kmalloc(sz) as *mut Self;
		unsafe {
			(*ptr).request = request;
		}
		ptr
	}
}

pub struct Request3<RqT, RmT, RpT> {
	request: RqT,
	mementries: RmT,
	response: RpT,
}

impl<RqT, RmT, RpT> Request3<RqT, RmT, RpT> {
	pub fn new(request: RqT, meminfo: RmT) -> *mut Self {
		let sz = size_of::<RqT>() + size_of::<RmT>() + size_of::<RpT>();
		let ptr = kmalloc(sz) as *mut Self;
		unsafe {
			(*ptr).request = request;
			(*ptr).mementries = meminfo;
		}
		ptr
	}
}

pub struct Device {
	queue:        *mut Queue,
	dev:          *mut u32,
	idx:          u16,
	ack_used_idx: u16,
	framebuffer:  *mut Pixel,
	width:        u32,
	height:       u32,
}

impl Device {
	pub const fn new() -> Self {
		Self { queue:        null_mut(),
		       dev:          null_mut(),
		       idx:          0,
			   ack_used_idx: 0, 
			   framebuffer:  null_mut(),
			   width: 640,
			   height: 480
		}
	}
}

static mut GPU_DEVICES: [Option<Device>; 8] = [
	None,
	None,
	None,
	None,
	None,
	None,
	None,
	None,
];

pub fn fill_rect(dev: &mut Device, rect: Rect, color: Pixel) {
	for row in rect.y..(rect.y+rect.height) {
		for col in rect.x..(rect.x+rect.width) {
			let byte = row as usize * dev.width as usize + col as usize;
			unsafe {
				dev.framebuffer.add(byte).write(color);
			}
		}
	}
}

pub fn stroke_rect(dev: &mut Device, rect: Rect, color: Pixel, size: u32) {
	// Essentially stroke the four sides.
	// Top
	fill_rect(dev, Rect::new(
		rect.x,
		rect.y,
		rect.width,
		size
	), color);
	// Bottom
	fill_rect(dev, Rect::new(
		rect.x,
		rect.y+rect.height,
		rect.width,
		size
	), color);
	// Left
	fill_rect(dev, Rect::new(
		rect.x,
		rect.y,
		size,
		rect.height
	), color);

	// Right
	fill_rect(dev, Rect::new(
		rect.x+rect.width,
		rect.y,
		size,
		rect.height+size
	), color);
}

fn look_mycos(angle_degrees: f64) -> f64 {
	const COS_TABLE: [f64; 73] = [
		1.0,
		0.9962,
		0.9848,
		0.9659,
		0.9397,
		0.9063,
		0.8660,
		0.8191,
		0.7660,
		0.7071,
		0.6428,
		0.5736,
		0.5000,
		0.4226,
		0.3420,
		0.2558,
		0.1736,
		0.0872,
		0.0,
		-0.0872,
		-0.1736,
		-0.2558,
		-0.3420,
		-0.4226,
		-0.5000,
		-0.5736,
		-0.6428,
		-0.7071,
		-0.7660,
		-0.8191,
		-0.8660,
		-0.9063,
		-0.9397,
		-0.9659,
		-0.9848,
		-0.9962,
		-1.0,
		-0.9962,
		-0.9848,
		-0.9659,
		-0.9397,
		-0.9063,
		-0.8660,
		-0.8191,
		-0.7660,
		-0.7071,
		-0.6428,
		-0.5736,
		-0.5000,
		-0.4226,
		-0.3420,
		-0.2558,
		-0.1736,
		-0.0872,
		0.0,
		0.0872,
		0.1736,
		0.2558,
		0.3420,
		0.4226,
		0.5000,
		0.5736,
		0.6428,
		0.7071,
		0.7660,
		0.8191,
		0.8660,
		0.9063,
		0.9397,
		0.9659,
		0.9848,
		0.9962,
		1.0,
	];
	let lookup_ang = angle_degrees as usize / 5;
	COS_TABLE[lookup_ang % COS_TABLE.len()]
}
fn fmod(x: f64, y: f64) -> f64 {
	let x = x as i64;
	let y = y as i64;
	(x - x / y * y) as f64
}

fn mycos(angle_degrees: f64) -> f64 {
	let angle_mod_360 = fmod(angle_degrees, 360.0);
	let x = 3.14159265359 * angle_mod_360 / 180.0;
	let mut result = 1.0;
	let mut inter = 1.0;
	let num = x * x;
	for i in 1..=10 {
		let comp = 2.0 * i as f64;
		let den = comp * (comp - 1.0);
		inter *= num / den;
		if i % 2 == 0 {
			result += inter;
		}
		else {
			result -= inter;
		}
	}
	result
}

fn mysin(angle_degrees: f64) -> f64 {
	mycos(90.0 - angle_degrees)
}

pub fn draw_cosine(dev: &mut Device, rect: Rect, color: Pixel) {
	for x in 1..=(rect.width-rect.x) {
		let fx = x as f64;
		let fy = -mycos(fx);
		let y = ((fy * rect.height as f64) as i32 + rect.y as i32) as u32;
		// println!("cos({}) is {}, gives y: {}", fx, fy, y);
		fill_rect(dev, Rect::new(
			rect.x + x,
			y,
			1, 1
		), color);
	}
}

pub fn init(gdev: usize)  {
	if let Some(mut dev) = unsafe { GPU_DEVICES[gdev-1].take() } {
		// Put some crap in the framebuffer:
		// First clear the buffer to white?
		fill_rect(&mut dev, Rect::new(0, 0, 640, 480), Pixel::new(255, 255, 255, 255));
		fill_rect(&mut dev, Rect::new(15, 15, 200, 200), Pixel::new(255, 130, 0, 255));
		stroke_rect(&mut dev, Rect::new( 255, 15, 150, 150), Pixel::new( 0, 0, 0, 255), 5);
		draw_cosine(&mut dev, Rect::new(0, 300, 550, 60), Pixel::new(255, 15, 15, 255));
		// //// STEP 1: Create a host resource using create 2d
		let rq = Request::new(ResourceCreate2d {
			hdr: CtrlHeader {
				ctrl_type: CtrlType::CmdResourceCreate2d,
				flags: 0,
				fence_id: 0,
				ctx_id: 0,
				padding: 0,
			},
			resource_id: 1,
			format: Formats::R8G8B8A8Unorm,
			width: dev.width,
			height: dev.height,
		});
		let desc_c2d = Descriptor {
			addr: unsafe { &(*rq).request as *const ResourceCreate2d as u64 },
			len: size_of::<ResourceCreate2d>() as u32,
			flags: VIRTIO_DESC_F_NEXT,
			next: (dev.idx + 1) % VIRTIO_RING_SIZE as u16,
		};
		let desc_c2d_resp = Descriptor {
			addr: unsafe { &(*rq).response as *const CtrlHeader as u64 },
			len: size_of::<CtrlHeader>() as u32,
			flags: VIRTIO_DESC_F_WRITE,
			next: 0,
		};
		unsafe {
			let head = dev.idx;
			(*dev.queue).desc[dev.idx as usize] = desc_c2d;
			dev.idx = (dev.idx + 1) % VIRTIO_RING_SIZE as u16;
			(*dev.queue).desc[dev.idx as usize] = desc_c2d_resp;
			dev.idx = (dev.idx + 1) % VIRTIO_RING_SIZE as u16;
			(*dev.queue).avail.ring[(*dev.queue).avail.idx as usize] = head;
			(*dev.queue).avail.idx =
				(*dev.queue).avail.idx.wrapping_add(1);
		}
		// //// STEP 2: Attach backing
		let rq = Request3::new(AttachBacking {
			hdr: CtrlHeader {
				ctrl_type: CtrlType::CmdResourceAttachBacking,
				flags: 0,
				fence_id: 0,
				ctx_id: 0,
				padding: 0,
			},
			resource_id: 1,
			nr_entries: 1,
		},
		MemEntry {
			addr: dev.framebuffer as u64,
			length: dev.width * dev.height * size_of::<Pixel>() as u32,
			padding: 0, 
		}
		);
		let desc_ab = Descriptor {
			addr: unsafe { &(*rq).request as *const AttachBacking as u64 },
			len: size_of::<AttachBacking>() as u32,
			flags: VIRTIO_DESC_F_NEXT,
			next: (dev.idx + 1) % VIRTIO_RING_SIZE as u16,
		};
		let desc_ab_mementry = Descriptor {
			addr: unsafe { &(*rq).mementries as *const MemEntry as u64 },
			len: size_of::<MemEntry>() as u32,
			flags: VIRTIO_DESC_F_NEXT,
			next: (dev.idx + 2) % VIRTIO_RING_SIZE as u16,
		};
		let desc_ab_resp = Descriptor {
			addr: unsafe { &(*rq).response as *const CtrlHeader as u64 },
			len: size_of::<CtrlHeader>() as u32,
			flags: VIRTIO_DESC_F_WRITE,
			next: 0,
		};
		unsafe {
			let head = dev.idx;
			(*dev.queue).desc[dev.idx as usize] = desc_ab;
			dev.idx = (dev.idx + 1) % VIRTIO_RING_SIZE as u16;
			(*dev.queue).desc[dev.idx as usize] = desc_ab_mementry;
			dev.idx = (dev.idx + 1) % VIRTIO_RING_SIZE as u16;
			(*dev.queue).desc[dev.idx as usize] = desc_ab_resp;
			dev.idx = (dev.idx + 1) % VIRTIO_RING_SIZE as u16;
			(*dev.queue).avail.ring[(*dev.queue).avail.idx as usize] = head;
			(*dev.queue).avail.idx =
				(*dev.queue).avail.idx.wrapping_add(1);
		}
		// //// STEP 3: Set scanout
		let rq = Request::new(SetScanout {
			hdr: CtrlHeader {
				ctrl_type: CtrlType::CmdSetScanout,
				flags: 0,
				fence_id: 0,
				ctx_id: 0,
				padding: 0,
			},
			r: Rect::new(0, 0, dev.width, dev.height),
			resource_id: 1,
			scanout_id: 0,
		});
		let desc_sso = Descriptor {
			addr: unsafe { &(*rq).request as *const SetScanout as u64 },
			len: size_of::<SetScanout>() as u32,
			flags: VIRTIO_DESC_F_NEXT,
			next: (dev.idx + 1) % VIRTIO_RING_SIZE as u16,
		};
		let desc_sso_resp = Descriptor {
			addr: unsafe { &(*rq).response as *const CtrlHeader as u64 },
			len: size_of::<CtrlHeader>() as u32,
			flags: VIRTIO_DESC_F_WRITE,
			next: 0,
		};
		unsafe {
			let head = dev.idx;
			(*dev.queue).desc[dev.idx as usize] = desc_sso;
			dev.idx = (dev.idx + 1) % VIRTIO_RING_SIZE as u16;
			(*dev.queue).desc[dev.idx as usize] = desc_sso_resp;
			dev.idx = (dev.idx + 1) % VIRTIO_RING_SIZE as u16;
			(*dev.queue).avail.ring[(*dev.queue).avail.idx as usize] = head;
			(*dev.queue).avail.idx =
				(*dev.queue).avail.idx.wrapping_add(1);
		}
		// //// STEP 4: Transfer to host
		let rq = Request::new(TransferToHost2d {
			hdr: CtrlHeader {
				ctrl_type: CtrlType::CmdTransferToHost2d,
				flags: 0,
				fence_id: 0,
				ctx_id: 0,
				padding: 0,
			},
			r: Rect::new(0, 0, dev.width, dev.height),
			offset: 0,
			resource_id: 1,
			padding: 0,
		});
		let desc_t2h = Descriptor {
			addr: unsafe { &(*rq).request as *const TransferToHost2d as u64 },
			len: size_of::<TransferToHost2d>() as u32,
			flags: VIRTIO_DESC_F_NEXT,
			next: (dev.idx + 1) % VIRTIO_RING_SIZE as u16,
		};
		let desc_t2h_resp = Descriptor {
			addr: unsafe { &(*rq).response as *const CtrlHeader as u64 },
			len: size_of::<CtrlHeader>() as u32,
			flags: VIRTIO_DESC_F_WRITE,
			next: 0,
		};
		unsafe {
			let head = dev.idx;
			(*dev.queue).desc[dev.idx as usize] = desc_t2h;
			dev.idx = (dev.idx + 1) % VIRTIO_RING_SIZE as u16;
			(*dev.queue).desc[dev.idx as usize] = desc_t2h_resp;
			dev.idx = (dev.idx + 1) % VIRTIO_RING_SIZE as u16;
			(*dev.queue).avail.ring[(*dev.queue).avail.idx as usize] = head;
			(*dev.queue).avail.idx =
				(*dev.queue).avail.idx.wrapping_add(1);
		}
		// Step 5: Flush
		let rq = Request::new(ResourceFlush {
			hdr: CtrlHeader {
				ctrl_type: CtrlType::CmdResourceFlush,
				flags: 0,
				fence_id: 0,
				ctx_id: 0,
				padding: 0,
			},
			r: Rect::new(0, 0, dev.width, dev.height),
			resource_id: 1,
			padding: 0,
		});
		let desc_rf = Descriptor {
			addr: unsafe { &(*rq).request as *const ResourceFlush as u64 },
			len: size_of::<ResourceFlush>() as u32,
			flags: VIRTIO_DESC_F_NEXT,
			next: (dev.idx + 1) % VIRTIO_RING_SIZE as u16,
		};
		let desc_rf_resp = Descriptor {
			addr: unsafe { &(*rq).response as *const CtrlHeader as u64 },
			len: size_of::<CtrlHeader>() as u32,
			flags: VIRTIO_DESC_F_WRITE,
			next: 0,
		};
		unsafe {
			let head = dev.idx;
			(*dev.queue).desc[dev.idx as usize] = desc_rf;
			dev.idx = (dev.idx + 1) % VIRTIO_RING_SIZE as u16;
			(*dev.queue).desc[dev.idx as usize] = desc_rf_resp;
			dev.idx = (dev.idx + 1) % VIRTIO_RING_SIZE as u16;
			(*dev.queue).avail.ring[(*dev.queue).avail.idx as usize] = head;
			(*dev.queue).avail.idx =
				(*dev.queue).avail.idx.wrapping_add(1);
		}
		// Run Queue
		unsafe {
			dev.dev
			.add(MmioOffsets::QueueNotify.scale32())
			.write_volatile(0);
			GPU_DEVICES[gdev-1].replace(dev);
		}
	}
}

pub fn setup_gpu_device(ptr: *mut u32) -> bool {
	unsafe {
		// We can get the index of the device based on its address.
		// 0x1000_1000 is index 0
		// 0x1000_2000 is index 1
		// ...
		// 0x1000_8000 is index 7
		// To get the number that changes over, we shift right 12 places (3 hex digits)
		let idx = (ptr as usize - virtio::MMIO_VIRTIO_START) >> 12;
		// [Driver] Device Initialization
		// 1. Reset the device (write 0 into status)
		ptr.add(MmioOffsets::Status.scale32()).write_volatile(0);
		let mut status_bits = StatusField::Acknowledge.val32();
		// 2. Set ACKNOWLEDGE status bit
		ptr.add(MmioOffsets::Status.scale32()).write_volatile(status_bits);
		// 3. Set the DRIVER status bit
		status_bits |= StatusField::DriverOk.val32();
		ptr.add(MmioOffsets::Status.scale32()).write_volatile(status_bits);
		// 4. Read device feature bits, write subset of feature
		// bits understood by OS and driver    to the device.
		let host_features = ptr.add(MmioOffsets::HostFeatures.scale32()).read_volatile();
		ptr.add(MmioOffsets::GuestFeatures.scale32()).write_volatile(host_features);
		// 5. Set the FEATURES_OK status bit
		status_bits |= StatusField::FeaturesOk.val32();
		ptr.add(MmioOffsets::Status.scale32()).write_volatile(status_bits);
		// 6. Re-read status to ensure FEATURES_OK is still set.
		// Otherwise, it doesn't support our features.
		let status_ok = ptr.add(MmioOffsets::Status.scale32()).read_volatile();
		// If the status field no longer has features_ok set,
		// that means that the device couldn't accept
		// the features that we request. Therefore, this is
		// considered a "failed" state.
		if false == StatusField::features_ok(status_ok) {
			print!("features fail...");
			ptr.add(MmioOffsets::Status.scale32()).write_volatile(StatusField::Failed.val32());
			return false;
		}
		// 7. Perform device-specific setup.
		// Set the queue num. We have to make sure that the
		// queue size is valid because the device can only take
		// a certain size.
		let qnmax = ptr.add(MmioOffsets::QueueNumMax.scale32()).read_volatile();
		ptr.add(MmioOffsets::QueueNum.scale32()).write_volatile(VIRTIO_RING_SIZE as u32);
		if VIRTIO_RING_SIZE as u32 > qnmax {
			print!("queue size fail...");
			return false;
		}
		// First, if the block device array is empty, create it!
		// We add 4095 to round this up and then do an integer
		// divide to truncate the decimal. We don't add 4096,
		// because if it is exactly 4096 bytes, we would get two
		// pages, not one.
		let num_pages = (size_of::<Queue>() + PAGE_SIZE - 1) / PAGE_SIZE;
		// println!("np = {}", num_pages);
		// We allocate a page for each device. This will the the
		// descriptor where we can communicate with the block
		// device. We will still use an MMIO register (in
		// particular, QueueNotify) to actually tell the device
		// we put something in memory. We also have to be
		// careful with memory ordering. We don't want to
		// issue a notify before all memory writes have
		// finished. We will look at that later, but we need
		// what is called a memory "fence" or barrier.
		ptr.add(MmioOffsets::QueueSel.scale32()).write_volatile(0);
		// TODO: Set up queue #1 (cursorq)

		// Alignment is very important here. This is the memory address
		// alignment between the available and used rings. If this is wrong,
		// then we and the device will refer to different memory addresses
		// and hence get the wrong data in the used ring.
		// ptr.add(MmioOffsets::QueueAlign.scale32()).write_volatile(2);
		let queue_ptr = zalloc(num_pages) as *mut Queue;
		let queue_pfn = queue_ptr as u32;
		ptr.add(MmioOffsets::GuestPageSize.scale32()).write_volatile(PAGE_SIZE as u32);
		// QueuePFN is a physical page number, however it
		// appears for QEMU we have to write the entire memory
		// address. This is a physical memory address where we
		// (the OS) and the block device have in common for
		// making and receiving requests.
		ptr.add(MmioOffsets::QueuePfn.scale32()).write_volatile(queue_pfn / PAGE_SIZE as u32);
		// 8. Set the DRIVER_OK status bit. Device is now "live"
		status_bits |= StatusField::DriverOk.val32();
		ptr.add(MmioOffsets::Status.scale32()).write_volatile(status_bits);


		let dev = Device {
			queue: queue_ptr,
			dev: ptr,
			idx: 0,
			ack_used_idx: 0,
			framebuffer: kmalloc(640*480*size_of::<Pixel>()) as *mut Pixel,
			width: 640,
			height: 480,
		};

		GPU_DEVICES[idx] = Some(dev);

		true
	}
}

pub fn pending(dev: &mut Device) {
	// Here we need to check the used ring and then free the resources
	// given by the descriptor id.
	unsafe {
		let ref queue = *dev.queue;
		while dev.ack_used_idx != queue.used.idx {
			
			let ref elem = queue.used.ring
				[dev.ack_used_idx as usize % VIRTIO_RING_SIZE];
			// println!("Ack {}, elem {}, len {}", dev.ack_used_idx, elem.id, elem.len);
			let ref desc = queue.desc[elem.id as usize];
			// Requests stay resident on the heap until this
			// function, so we can recapture the address here
			kfree(desc.addr as *mut u8);
			dev.ack_used_idx = dev.ack_used_idx.wrapping_add(1);

		}
	}
}

pub fn handle_interrupt(idx: usize) {
	unsafe {
		if let Some(bdev) = GPU_DEVICES[idx].as_mut() {
			pending(bdev);
		}
		else {
			println!(
			         "Invalid GPU device for interrupt {}",
			         idx + 1
			);
		}
	}
}
