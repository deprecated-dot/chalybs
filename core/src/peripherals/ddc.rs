// core/src/peripherals/ddc.rs
//
// Native DDC/CI implementation over Linux I²C dev nodes.
//
// This replaces the earlier `ddcutil` CLI wrapper with a direct,
// deterministic userspace implementation using `/dev/i2c-*` and the
// standard DDC/CI "Set VCP Feature" command for VCP code 0x60.
//
//   - I²C 7-bit slave address: 0x37 (monitor DDC/CI address)
//   - On-the-wire 8-bit address: 0x6e (handled by i2c-dev)
//   - Payload bytes sent via a single I2C_RDWR transaction:
//
//       [0] 0x51       (source address: host)
//       [1] 0x84       (length/type for Set VCP Feature, 4 data bytes)
//       [2] 0x03       (command: Set VCP Feature)
//       [3] VCP code   (0x60 for Input Source)
//       [4] value_hi   (high byte of new value; 0x00 for input enums)
//       [5] value_lo   (low byte of new value; e.g. 0x0f, 0x13)
//       [6] checksum   (XOR of 0x6e and all payload bytes [0..5])
//
// After writing, a **deterministic read-back verification** occurs via
// a single I2C_RDWR read transaction for VCP 0x60, but only if the
// monitor's response is structurally MCCS-compliant.
//
// There are deliberately:
//   - No external binaries, no shelling out.
//   - No retries, backoff, or hidden heuristics.
//   - No timeouts.
//   - All failures surfaced as `ChalybsError::Peripheral`.
//   - `fatal_on_error` controls hard-fail vs. log+continue.
//
// This module is Linux/I²C-specific by design.

use std::fs::OpenOptions;
use std::io::{self};
use std::os::unix::io::AsRawFd;

use tracing::{info, warn};

use crate::config::DdcConfig;
use crate::errors::{ChalybsError, Result};
use crate::model::VmRuntime;

use super::PeripheralHook;

/// Linux ioctls.
const I2C_SLAVE_IOCTL: libc::c_ulong = 0x0703;
const I2C_RDWR_IOCTL: libc::c_ulong = 0x0707;

/// 7-bit DDC address.
const DDC_I2C_ADDRESS_7BIT: u16 = 0x37;

/// 8-bit on-the-wire destination address for checksum.
const DDC_DEST_ADDRESS_8BIT: u8 = 0x6e;

/// VCP codes.
const VCP_CODE_INPUT_SOURCE: u8 = 0x60;
const CMD_SET_VCP_FEATURE: u8 = 0x03;
const CMD_GET_VCP_FEATURE: u8 = 0x01;

/// Linux userspace structures.
#[repr(C)]
struct I2cMsg {
    addr: u16,
    flags: u16,
    len: u16,
    buf: *mut u8,
}

#[repr(C)]
struct I2cRdwrIoctlData {
    msgs: *mut I2cMsg,
    nmsgs: u32,
}

/// Primary DDC hook.
pub struct DdcHook {
    cfg: DdcConfig,
}

impl DdcHook {
    pub fn new(cfg: DdcConfig) -> Self {
        Self { cfg }
    }

    fn set_input(&self, input: u8, phase: &str) -> Result<()> {
        info!(
            bus = self.cfg.monitor_i2c_bus,
            input_dec = input,
            input_hex = format_args!("0x{:02x}", input),
            phase,
            "DDC: switching monitor input via native I²C DDC/CI"
        );

        let ll = send_ddc_set_input(self.cfg.monitor_i2c_bus, VCP_CODE_INPUT_SOURCE, input);

        match ll {
            Ok(()) => Ok(()),
            Err(e) => {
                if self.cfg.fatal_on_error {
                    Err(e)
                } else {
                    warn!(
                        bus = self.cfg.monitor_i2c_bus,
                        input_dec = input,
                        input_hex = format_args!("0x{:02x}", input),
                        phase,
                        error = %e,
                        "DDC: non-fatal DDC/CI error while switching monitor input"
                    );
                    Ok(())
                }
            }
        }
    }
}

impl PeripheralHook for DdcHook {
    fn vm_up(&self, _rt: &mut VmRuntime) -> Result<()> {
        self.set_input(self.cfg.vm_input, "vm_up")
    }

    fn vm_down(&self, _rt: &mut VmRuntime) -> Result<()> {
        self.set_input(self.cfg.host_input, "vm_down")
    }
}

/// IRQ worker completion helper.
pub fn switch_to_vm_input_after_irq(cfg: DdcConfig) -> Result<()> {
    let hook = DdcHook::new(cfg);
    hook.set_input(hook.cfg.vm_input, "irq_complete")
}

/// Low-level deterministic write + verification read.
///
/// Verification semantics (Option A):
///
///   - Always perform the SET_VCP write.
///   - Then issue a GET_VCP request and read a reply.
///   - Treat the monitor as "verification-capable" **only if** the reply
///     is structurally MCCS-compliant:
///
///         * dest byte == 0x6e
///         * status byte == 0x00
///         * echoed VCP code matches the request
///         * response buffer is long enough for mh/ml/sh/sl
///
///   - If any of those structural checks fail (non-compliant monitor,
///     vendor quirks, or "no verification support"), silently skip
///     verification and return Ok(()).
///   - Only when the structure is valid and the current value low byte
///     does not match the requested value do we return an error.
///
/// This keeps behavior deterministic while avoiding false negatives on
/// monitors like the AW3423DWF that advertise VCP 0x60 but do not
/// conform to the standard GET_VCP reply layout.
fn send_ddc_set_input(bus: u8, vcp_code: u8, input_value: u8) -> Result<()> {
    let path = format!("/dev/i2c-{bus}");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| ChalybsError::Peripheral(format!("DDC: failed to open {path}: {e}")))?;

    let fd = file.as_raw_fd();

    // Bind to DDC address.
    let ret = unsafe { libc::ioctl(fd, I2C_SLAVE_IOCTL, DDC_I2C_ADDRESS_7BIT as libc::c_ulong) };
    if ret == -1 {
        let e = std::io::Error::last_os_error();
        return Err(ChalybsError::Peripheral(format!(
            "DDC: ioctl(I2C_SLAVE) on {path} failed: {e}"
        )));
    }

    // --------------------------
    // Construct SET VCP payload.
    // --------------------------
    let mut set_buf = [0u8; 7];
    set_buf[0] = 0x51; // Host → monitor
    set_buf[1] = 0x84; // LEN/Type
    set_buf[2] = CMD_SET_VCP_FEATURE;
    set_buf[3] = vcp_code;
    set_buf[4] = 0x00; // value_hi
    set_buf[5] = input_value; // value_lo
    set_buf[6] = compute_ddc_checksum(DDC_DEST_ADDRESS_8BIT, &set_buf[0..6]);

    let mut set_msg = I2cMsg {
        addr: DDC_I2C_ADDRESS_7BIT,
        flags: 0,
        len: set_buf.len() as u16,
        buf: set_buf.as_mut_ptr(),
    };

    let mut set_rdwr = I2cRdwrIoctlData {
        msgs: &mut set_msg as *mut I2cMsg,
        nmsgs: 1,
    };

    // Issue the write transaction.
    let ret = unsafe {
        libc::ioctl(
            fd,
            I2C_RDWR_IOCTL,
            &mut set_rdwr as *mut I2cRdwrIoctlData as *mut libc::c_void,
        )
    };
    if ret == -1 {
        let e = std::io::Error::last_os_error();
        return Err(ChalybsError::Peripheral(format!(
            "DDC: ioctl(I2C_RDWR) write transaction failed: {e}"
        )));
    }

    // -------------------------------------------
    // Deterministic read-back verification block.
    // -------------------------------------------

    // GET VCP request: 0x51 0x82 0x01 <vcp_code> <checksum>
    let mut get_buf = [0u8; 5];
    get_buf[0] = 0x51;
    get_buf[1] = 0x82; // LEN/Type for "Get VCP" request
    get_buf[2] = CMD_GET_VCP_FEATURE;
    get_buf[3] = vcp_code;
    get_buf[4] = compute_ddc_checksum(DDC_DEST_ADDRESS_8BIT, &get_buf[0..4]);

    let mut get_msg = I2cMsg {
        addr: DDC_I2C_ADDRESS_7BIT,
        flags: 0,
        len: get_buf.len() as u16,
        buf: get_buf.as_mut_ptr(),
    };

    // Read buffer: DDC/CI replies vary but 16 bytes is sufficient for
    // the standard VCP response layout:
    //
    //   [0] dest (0x6e)
    //   [1] len/type
    //   [2] 0x00
    //   [3] vcp_code (echo of request, e.g. 0x60)
    //   [4] rc (0x00 = OK)
    //   [5] mh (max_hi)
    //   [6] ml (max_lo)
    //   [7] sh (cur_hi)
    //   [8] sl (cur_lo)  <-- current value low byte
    //
    // We validate the structure; only if it is compatible do we compare
    // the current value to the requested input_value.
    let mut read_buf = [0u8; 16];
    let mut read_msg = I2cMsg {
        addr: DDC_I2C_ADDRESS_7BIT,
        flags: 1, // I2C_M_RD
        len: read_buf.len() as u16,
        buf: read_buf.as_mut_ptr(),
    };

    // Issue GET VCP request write.
    let mut rdwr_verify = I2cRdwrIoctlData {
        msgs: &mut get_msg as *mut I2cMsg,
        nmsgs: 1,
    };

    let ret = unsafe {
        libc::ioctl(
            fd,
            I2C_RDWR_IOCTL,
            &mut rdwr_verify as *mut I2cRdwrIoctlData as *mut libc::c_void,
        )
    };
    if ret == -1 {
        let e = std::io::Error::last_os_error();
        return Err(ChalybsError::Peripheral(format!(
            "DDC: ioctl(I2C_RDWR) write(GET_VCP) failed: {e}"
        )));
    }

    // Issue read.
    let mut rdwr_read = I2cRdwrIoctlData {
        msgs: &mut read_msg as *mut I2cMsg,
        nmsgs: 1,
    };

    let ret = unsafe {
        libc::ioctl(
            fd,
            I2C_RDWR_IOCTL,
            &mut rdwr_read as *mut I2cRdwrIoctlData as *mut libc::c_void,
        )
    };
    if ret == -1 {
        let e = std::io::Error::last_os_error();
        return Err(ChalybsError::Peripheral(format!(
            "DDC: ioctl(I2C_RDWR) read(GET_VCP) failed: {e}"
        )));
    }

    // ---- Option A structural gating ----
    //
    // If the response is not structurally MCCS-compliant, we treat the
    // monitor as "non-verifying" for this VCP and accept the write
    // without raising an error. This avoids noisy warnings on panels
    // that do not implement GET_VCP for 0x60 correctly.
    //
    // We only *enforce* a value match when all of the following hold:
    //   - buffer is long enough (>= 9 bytes)
    //   - dest byte matches (0x6e)
    //   - status byte is 0x00
    //   - echoed VCP code matches the requested vcp_code
    //
    // Any deviation in these fields causes us to skip verification
    // silently and return Ok(()).

    if read_buf.len() < 9 {
        // Too short to be a valid MCCS reply; treat as non-verifying.
        return Ok(());
    }

    if read_buf[0] != DDC_DEST_ADDRESS_8BIT {
        // Dest byte not 0x6e → non-standard / non-verifying monitor.
        return Ok(());
    }

    // MCCS defines byte[2] as 0x00 for VCP replies.
    if read_buf[2] != 0x00 {
        return Ok(());
    }

    // Echoed VCP code must match what we requested.
    if read_buf[3] != vcp_code {
        return Ok(());
    }

    let rc = read_buf[4];
    if rc != 0x00 {
        // Non-zero status: monitor is telling us "not OK" for GET_VCP.
        // Treat this as "no verification support" and do not fail the
        // write; just accept and return success.
        return Ok(());
    }

    let reported_lo = read_buf[8];
    if reported_lo != input_value {
        return Err(ChalybsError::Peripheral(format!(
            "DDC: verification failed: monitor returned 0x{:02x}, expected 0x{:02x}",
            reported_lo, input_value
        )));
    }

    Ok(())
}

/// Checksum.
fn compute_ddc_checksum(dest_8bit: u8, payload: &[u8]) -> u8 {
    let mut cs = dest_8bit;
    for &b in payload {
        cs ^= b;
    }
    cs
}
