pub const HOST: &str = "127.0.0.1";
pub const PORT: u16 = 3814;
pub const WEB_ORIGIN: &str = "https://web.monsgeek.com";
pub const HIDRAW_SYSFS: &str = "/sys/class/hidraw";
pub const MONSGEEK_VENDOR: &str = "3151";
pub const MONSGEEK_PRODUCT: &str = "502d";

pub const GET_INFOR: u8 = 0x8f;
pub const CHECKSUM_BIT7: u32 = 0;
pub const CHECKSUM_BIT8: u32 = 1;
pub const LIGHT_OTHER: u32 = 2;
pub const FEA_CMD_SET_MAGNETISM_REPORT: u8 = 0x1b;
pub const FEA_CMD_SET_MAGNETISM_CAL: u8 = 0x1c;
pub const FEA_CMD_SET_MAGNETISM_CALMAX: u8 = 0x1e;
pub const FEA_CMD_GET_MAGNETISM_BY_ARR: u8 = 0xe5;
pub const MAGNETISM_TRAVEL_VALUES: u8 = 0xfe;

pub const CALIBRATION_KEYS_PER_PAGE: usize = 32;
pub const CALIBRATION_MAX_KEYS: usize = CALIBRATION_KEYS_PER_PAGE * 4;
pub const REPORT_BYTES: usize = 64;
