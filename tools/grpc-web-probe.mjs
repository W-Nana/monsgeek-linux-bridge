#!/usr/bin/env node

import http from "node:http";
import { execFileSync } from "node:child_process";
import {
  closeSync,
  constants as fsConstants,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  realpathSync,
  renameSync,
  statfsSync,
  writeFileSync,
} from "node:fs";
import { freemem, homedir, loadavg, totalmem } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const HOST = "127.0.0.1";
const PORT = 3814;
const WEB_ORIGIN = "https://web.monsgeek.com";
const HIDRAW_SYSFS = "/sys/class/hidraw";
const MONSGEEK_VENDOR = "3151";
const MONSGEEK_PRODUCT = "502d";
const FEATURE_IO = process.env.MONSGEEK_FEATURE_IO ?? "/tmp/hid-feature-io";
const FEATURE_IO_SOURCE = fileURLToPath(new URL("./hid-feature-io.c", import.meta.url));
const DEFAULT_STATE_DIR = process.env.XDG_STATE_HOME ?? join(homedir(), ".local", "state");
const DB_FILE =
  process.env.MONSGEEK_DB_FILE ?? join(DEFAULT_STATE_DIR, "monsgeek-linux-bridge", "db.json");
const TRACE_HID_REPORTS = process.env.MONSGEEK_TRACE_HID === "1";
const LIVE_WEATHER = process.env.MONSGEEK_LIVE_WEATHER === "1";
const WEATHER_ENDPOINT = "http://w2.yiketianqi.com/";
const GET_INFOR = 0x8f;
const CHECKSUM_BIT7 = 0;
const CHECKSUM_BIT8 = 1;
const LIGHT_MUSIC2 = 0;
const LIGHT_SCREEN = 1;
const LIGHT_OTHER = 2;
const FEA_CMD_SET_LEDPARAMS = new Set([0x04, 0x06, 0x07]);
const FEA_CMD_SET_MAGNETISM_REPORT = 0x1b;
const FEA_CMD_SET_MAGNETISM_CAL = 0x1c;
const FEA_CMD_SET_MAGNETISM_CALMAX = 0x1e;
const FEA_CMD_GET_MAGNETISM_BY_ARR = 0xe5;
const MAGNETISM_TRAVEL_VALUES = 0xfe;
const VENDOR_INPUT_READER = process.env.MONSGEEK_VENDOR_INPUT_READER !== "0";
const CALIBRATION_HOLD = process.env.MONSGEEK_CALIBRATION_HOLD === "1";
const SYNTHETIC_SIMULATION = process.env.MONSGEEK_SYNTHETIC_SIMULATION === "1";
const SYNTHETIC_TRAVEL_MAX = Number.parseInt(process.env.MONSGEEK_SIM_TRAVEL_MAX ?? "400", 10);
let microphoneMuted = false;
const deviceStates = new Map();
const streamClients = {
  watchDevList: new Set(),
  watchVender: new Set(),
  watchSystemInfo: new Set(),
};
const vendorInputReaders = new Map();

function readSysfsText(file) {
  try {
    return readFileSync(file, "utf8").trim();
  } catch {
    return undefined;
  }
}

function readSysfsBytes(file) {
  try {
    return readFileSync(file);
  } catch {
    return undefined;
  }
}

function usbParent(start) {
  let current = start;
  for (;;) {
    const vendor = readSysfsText(join(current, "idVendor"));
    const product = readSysfsText(join(current, "idProduct"));
    if (vendor && product) {
      return { vendor: vendor.toLowerCase(), product: product.toLowerCase() };
    }

    const parent = dirname(current);
    if (parent === current || parent === "/sys") {
      return undefined;
    }
    current = parent;
  }
}

function isVendorFeatureInterface(descriptor) {
  if (!descriptor) {
    return false;
  }

  const bytes = descriptor.toString("hex");
  return bytes.includes("06ffff") && bytes.includes("9540") && bytes.includes("7508") && bytes.includes("b1");
}

function detectedHidraw() {
  if (process.env.MONSGEEK_HIDRAW) {
    return process.env.MONSGEEK_HIDRAW;
  }

  try {
    for (const name of readdirSync(HIDRAW_SYSFS).sort()) {
      if (!name.startsWith("hidraw")) {
        continue;
      }

      const sysfs = realpathSync(join(HIDRAW_SYSFS, name, "device"));
      const usb = usbParent(sysfs);
      const descriptor = readSysfsBytes(join(HIDRAW_SYSFS, name, "device", "report_descriptor"));
      if (
        usb?.vendor === MONSGEEK_VENDOR &&
        usb.product === MONSGEEK_PRODUCT &&
        isVendorFeatureInterface(descriptor)
      ) {
        return `/dev/${name}`;
      }
    }
  } catch {
    // Fall through to the historical development default below.
  }

  return "/dev/hidraw4";
}

const DEFAULT_HIDRAW = detectedHidraw();

function encodeVarint(value) {
  const bytes = [];
  let rest = BigInt(value);
  while (rest > 0x7fn) {
    bytes.push(Number((rest & 0x7fn) | 0x80n));
    rest >>= 7n;
  }
  bytes.push(Number(rest));
  return Buffer.from(bytes);
}

function protobufString(field, value) {
  const bytes = Buffer.from(value, "utf8");
  return Buffer.concat([encodeVarint((field << 3) | 2), encodeVarint(bytes.length), bytes]);
}

function protobufVarint(field, value) {
  return Buffer.concat([encodeVarint(field << 3), encodeVarint(value)]);
}

function protobufBytes(field, value) {
  return Buffer.concat([encodeVarint((field << 3) | 2), encodeVarint(value.length), value]);
}

function protobufFloat(field, value) {
  const bytes = Buffer.alloc(4);
  bytes.writeFloatLE(value, 0);
  return Buffer.concat([encodeVarint((field << 3) | 5), bytes]);
}

function grpcFrame(flag, payload) {
  const header = Buffer.alloc(5);
  header[0] = flag;
  header.writeUInt32BE(payload.length, 1);
  return Buffer.concat([header, payload]);
}

function grpcTextResponse(message, status = 0, statusMessage = "") {
  const trailers = Buffer.from(
    `grpc-status:${status}\r\ngrpc-message:${encodeURIComponent(statusMessage)}\r\n`,
    "ascii",
  );
  const frames = [grpcFrame(0x00, message), grpcFrame(0x80, trailers)];
  return Buffer.concat(frames).toString("base64");
}

function writeGrpcWebMessage(response, message) {
  response.write(grpcFrame(0x00, message).toString("base64"));
}

function attachStream(method, response, producer, intervalMs) {
  response.writeHead(200, headers());
  const clients = streamClients[method];
  clients?.add(response);

  const emit = () => {
    try {
      writeGrpcWebMessage(response, producer());
    } catch (error) {
      writeGrpcWebMessage(response, Buffer.alloc(0));
      console.warn(`${method} stream producer failed: ${error.message}`);
    }
  };

  emit();
  const timer = setInterval(emit, intervalMs);
  const close = () => {
    clearInterval(timer);
    clients?.delete(response);
  };
  response.on("close", close);
  response.on("finish", close);
}

function broadcastStream(method, message) {
  for (const response of streamClients[method] ?? []) {
    writeGrpcWebMessage(response, message);
  }
}

function startVendorInputReader(devicePath = DEFAULT_HIDRAW) {
  if (!VENDOR_INPUT_READER || vendorInputReaders.has(devicePath)) {
    return;
  }

  let fd;
  try {
    fd = openSync(devicePath, fsConstants.O_RDONLY | fsConstants.O_NONBLOCK);
  } catch (error) {
    console.warn(`vendor input reader disabled for ${devicePath}: ${error.message}`);
    return;
  }

  const buffer = Buffer.alloc(64);
  const reader = {
    fd,
    timer: setInterval(() => {
      for (;;) {
        let bytesRead = 0;
        try {
          bytesRead = readSync(fd, buffer, 0, buffer.length, null);
        } catch (error) {
          if (error.code === "EAGAIN" || error.code === "EWOULDBLOCK") {
            break;
          }
          console.warn(`vendor input read failed for ${devicePath}: ${error.message}`);
          break;
        }
        if (bytesRead <= 0) {
          break;
        }

        const report = Buffer.from(buffer.subarray(0, bytesRead));
        if (TRACE_HID_REPORTS) {
          console.log(`vendor input ${devicePath} ${report.toString("hex")}`);
        }
        broadcastStream("watchVender", venderMessage(report));
      }
    }, 10),
  };
  vendorInputReaders.set(devicePath, reader);
  console.log(`Started vendor input reader for ${devicePath}`);
}

function stopVendorInputReaders() {
  for (const [devicePath, reader] of vendorInputReaders) {
    clearInterval(reader.timer);
    try {
      closeSync(reader.fd);
    } catch {
      // File descriptor is already closed.
    }
    vendorInputReaders.delete(devicePath);
  }
}

function requestPayload(body) {
  let raw;
  try {
    raw = Buffer.from(body.toString("ascii"), "base64");
  } catch {
    return "invalid grpc-web-text";
  }

  if (raw.length < 5) {
    return `short body ${raw.toString("hex")}`;
  }

  const flag = raw[0];
  const length = raw.readUInt32BE(1);
  const payloadHex = raw.subarray(5, 5 + length).toString("hex");
  const suffix = payloadHex.length > 256 ? "..." : "";
  return `frame=0x${flag.toString(16)} bytes=${length} payload=${payloadHex.slice(0, 256)}${suffix}`;
}

function decodeGrpcTextBody(body) {
  const raw = Buffer.from(body.toString("ascii"), "base64");
  if (raw.length < 5 || raw[0] !== 0x00) {
    return Buffer.alloc(0);
  }
  return raw.subarray(5, 5 + raw.readUInt32BE(1));
}

function skipField(bytes, index, wireType) {
  if (wireType === 0) {
    while (index < bytes.length && bytes[index++] & 0x80) {
      // Skip a varint.
    }
    return index;
  }
  if (wireType === 2) {
    const length = decodeVarint(bytes, index);
    return length.next + length.value;
  }
  throw new Error(`unsupported protobuf wire type ${wireType}`);
}

function decodeVarint(bytes, index) {
  let value = 0;
  let shift = 0;
  while (index < bytes.length) {
    const byte = bytes[index++];
    value |= (byte & 0x7f) << shift;
    if ((byte & 0x80) === 0) {
      return { value, next: index };
    }
    shift += 7;
  }
  throw new Error("truncated protobuf varint");
}

function decodeBytes(bytes, index) {
  const length = decodeVarint(bytes, index);
  return {
    value: bytes.subarray(length.next, length.next + length.value),
    next: length.next + length.value,
  };
}

function decodeSendMsg(bytes) {
  const message = {
    devicePath: DEFAULT_HIDRAW,
    payload: Buffer.alloc(0),
    checksumType: CHECKSUM_BIT7,
    dangleDevType: 0,
  };
  for (let index = 0; index < bytes.length; ) {
    const key = decodeVarint(bytes, index);
    index = key.next;
    const field = key.value >> 3;
    const wireType = key.value & 0x07;
    if ((field === 1 || field === 2) && wireType === 2) {
      const value = decodeBytes(bytes, index);
      index = value.next;
      if (field === 1) {
        message.devicePath = value.value.toString("utf8");
      } else {
        message.payload = value.value;
      }
      continue;
    }
    if (field === 3 && wireType === 0) {
      const value = decodeVarint(bytes, index);
      message.checksumType = value.value;
      index = value.next;
      continue;
    }
    if (field === 4 && wireType === 0) {
      const value = decodeVarint(bytes, index);
      message.dangleDevType = value.value;
      index = value.next;
      continue;
    }
    index = skipField(bytes, index, wireType);
  }
  return message;
}

function decodeSetLight(bytes) {
  const message = {
    devicePath: DEFAULT_HIDRAW,
    lightType: LIGHT_OTHER,
    screenId: 0,
    dangleDevType: 0,
  };
  for (let index = 0; index < bytes.length; ) {
    const key = decodeVarint(bytes, index);
    index = key.next;
    const field = key.value >> 3;
    const wireType = key.value & 0x07;
    if (field === 1 && wireType === 2) {
      const value = decodeBytes(bytes, index);
      message.devicePath = value.value.toString("utf8");
      index = value.next;
      continue;
    }
    if ((field === 2 || field === 3 || field === 4) && wireType === 0) {
      const value = decodeVarint(bytes, index);
      if (field === 2) {
        message.lightType = value.value;
      } else if (field === 3) {
        message.screenId = value.value;
      } else {
        message.dangleDevType = value.value;
      }
      index = value.next;
      continue;
    }
    index = skipField(bytes, index, wireType);
  }
  return message;
}

function decodeReadMsg(bytes) {
  let devicePath = DEFAULT_HIDRAW;
  for (let index = 0; index < bytes.length; ) {
    const key = decodeVarint(bytes, index);
    index = key.next;
    const field = key.value >> 3;
    const wireType = key.value & 0x07;
    if (field === 1 && wireType === 2) {
      const value = decodeBytes(bytes, index);
      devicePath = value.value.toString("utf8");
      index = value.next;
      continue;
    }
    index = skipField(bytes, index, wireType);
  }
  return { devicePath };
}

function decodeDbMessage(bytes) {
  const message = { dbpath: "", key: Buffer.alloc(0), value: Buffer.alloc(0) };
  for (let index = 0; index < bytes.length; ) {
    const key = decodeVarint(bytes, index);
    index = key.next;
    const field = key.value >> 3;
    const wireType = key.value & 0x07;
    if (wireType === 2 && field >= 1 && field <= 3) {
      const fieldValue = decodeBytes(bytes, index);
      index = fieldValue.next;
      if (field === 1) {
        message.dbpath = fieldValue.value.toString("utf8");
      } else if (field === 2) {
        message.key = fieldValue.value;
      } else {
        message.value = fieldValue.value;
      }
      continue;
    }
    index = skipField(bytes, index, wireType);
  }
  return message;
}

function decodeBoolField(bytes, targetField, defaultValue = false) {
  for (let index = 0; index < bytes.length; ) {
    const key = decodeVarint(bytes, index);
    index = key.next;
    const field = key.value >> 3;
    const wireType = key.value & 0x07;
    if (field === targetField && wireType === 0) {
      const value = decodeVarint(bytes, index);
      return value.value !== 0;
    }
    index = skipField(bytes, index, wireType);
  }
  return defaultValue;
}

function decodeWeatherReq(bytes) {
  const message = { language: "", address: "" };
  for (let index = 0; index < bytes.length; ) {
    const key = decodeVarint(bytes, index);
    index = key.next;
    const field = key.value >> 3;
    const wireType = key.value & 0x07;
    if ((field === 1 || field === 2) && wireType === 2) {
      const value = decodeBytes(bytes, index);
      index = value.next;
      if (field === 1) {
        message.language = value.value.toString("utf8");
      } else {
        message.address = value.value.toString("utf8");
      }
      continue;
    }
    index = skipField(bytes, index, wireType);
  }
  return message;
}

function decodeOtaUpgrade(bytes) {
  const message = { devicePath: DEFAULT_HIDRAW, fileBytes: 0 };
  for (let index = 0; index < bytes.length; ) {
    const key = decodeVarint(bytes, index);
    index = key.next;
    const field = key.value >> 3;
    const wireType = key.value & 0x07;
    if (field === 1 && wireType === 2) {
      const value = decodeBytes(bytes, index);
      message.devicePath = value.value.toString("utf8");
      index = value.next;
      continue;
    }
    if (field === 2 && wireType === 2) {
      const value = decodeBytes(bytes, index);
      message.fileBytes = value.value.length;
      index = value.next;
      continue;
    }
    index = skipField(bytes, index, wireType);
  }
  return message;
}

function normalizedPayload(payload, checksumType) {
  const report = Buffer.alloc(64);
  payload.copy(report, 0, 0, report.length);
  if (checksumType === CHECKSUM_BIT7) {
    let sum = 0;
    for (let index = 0; index < 7; index += 1) {
      sum += report[index];
    }
    report[7] = 0xff - (sum & 0xff);
  } else if (checksumType === CHECKSUM_BIT8) {
    let sum = 0;
    for (let index = 0; index < 8; index += 1) {
      sum += report[index];
    }
    report[8] = 0xff - (sum & 0xff);
  }
  return report;
}

function stateFor(devicePath) {
  const key = devicePath || DEFAULT_HIDRAW;
  if (!deviceStates.has(key)) {
    deviceStates.set(key, {
      lastCommand: undefined,
      lastLedParam: undefined,
      light: { lightType: LIGHT_OTHER, screenId: 0, dangleDevType: 0 },
      calibration: {
        active: false,
        maximum: false,
        travelReads: 0,
        maxPages: new Map(),
      },
      magnetismReport: {
        active: false,
        timer: undefined,
        phase: 0,
      },
    });
  }
  return deviceStates.get(key);
}

function lightTypeName(lightType) {
  if (lightType === LIGHT_MUSIC2) {
    return "MUSIC2";
  }
  if (lightType === LIGHT_SCREEN) {
    return "SCREEN";
  }
  if (lightType === LIGHT_OTHER) {
    return "OTHER";
  }
  return `UNKNOWN_${lightType}`;
}

function updateWriteState(message, report) {
  const state = stateFor(message.devicePath);
  state.lastCommand = report[0];
  if (FEA_CMD_SET_LEDPARAMS.has(report[0])) {
    state.lastLedParam = Buffer.from(report);
    broadcastStream("watchVender", venderMessage(Buffer.from([0x00, 0x04, report[1] & 0xff, 0x00])));
  } else if (report[0] === FEA_CMD_SET_MAGNETISM_REPORT) {
    state.magnetismReport.active = report[1] !== 0;
    if (state.magnetismReport.active) {
      startVendorInputReader(message.devicePath || DEFAULT_HIDRAW);
    }
    if (state.magnetismReport.active && SYNTHETIC_SIMULATION) {
      startMagnetismReport(message.devicePath, state);
    } else {
      stopMagnetismReport(state);
      emitMagnetTravel(0, 0);
    }
  } else if (report[0] === FEA_CMD_SET_MAGNETISM_CAL) {
    state.calibration.active = report[1] !== 0;
    if (state.calibration.active) {
      state.calibration.maxPages.clear();
    } else {
      resetCalibrationState(state);
    }
  } else if (report[0] === FEA_CMD_SET_MAGNETISM_CALMAX) {
    state.calibration.maximum = report[1] !== 0;
    if (state.calibration.maximum) {
      state.calibration.maxPages.clear();
    } else {
      resetCalibrationState(state);
    }
  } else if (report[0] === FEA_CMD_GET_MAGNETISM_BY_ARR) {
    state.calibration.travelReads += 1;
    state.lastMagnetismRead = { kind: report[1], page: report[3] };
  }
}

function venderMessage(payload) {
  return payload.length > 0 ? protobufBytes(1, payload) : Buffer.alloc(0);
}

function emitMagnetTravel(travel, keyIndex = 0) {
  const clampedTravel = Math.max(0, Math.min(0xffff, Math.round(travel)));
  broadcastStream(
    "watchVender",
    venderMessage(
      Buffer.from([
        0x00,
        FEA_CMD_SET_MAGNETISM_REPORT,
        clampedTravel & 0xff,
        (clampedTravel >> 8) & 0xff,
        keyIndex & 0xff,
      ]),
    ),
  );
}

function startMagnetismReport(devicePath, state) {
  if (state.magnetismReport.timer) {
    return;
  }

  state.magnetismReport.timer = setInterval(() => {
    if (!state.magnetismReport.active) {
      stopMagnetismReport(state);
      return;
    }

    state.magnetismReport.phase = (state.magnetismReport.phase + 1) % 140;
    const phase = state.magnetismReport.phase;
    let travel;
    if (phase < 35) {
      travel = Math.round((phase / 34) * SYNTHETIC_TRAVEL_MAX);
    } else if (phase < 85) {
      travel = SYNTHETIC_TRAVEL_MAX;
    } else {
      travel = Math.round(((139 - phase) / 54) * SYNTHETIC_TRAVEL_MAX);
    }
    stateFor(devicePath).magnetismReport.phase = state.magnetismReport.phase;
    emitMagnetTravel(travel, 0);
  }, 50);
}

function stopMagnetismReport(state) {
  if (state.magnetismReport.timer) {
    clearInterval(state.magnetismReport.timer);
    state.magnetismReport.timer = undefined;
  }
  state.magnetismReport.phase = 0;
}

function resetCalibrationState(state) {
  state.calibration.active = false;
  state.calibration.maximum = false;
  state.calibration.travelReads = 0;
  state.calibration.maxPages.clear();
  state.lastMagnetismRead = undefined;
}

function monotonicCalibrationPayload(state, payload) {
  const query = state.lastMagnetismRead;
  if (
    !CALIBRATION_HOLD ||
    !state.calibration.maximum ||
    !query ||
    query.kind !== MAGNETISM_TRAVEL_VALUES ||
    payload.length < 2
  ) {
    return payload;
  }

  const page = query.page ?? 0;
  const maxPayload = state.calibration.maxPages.get(page) ?? Buffer.alloc(payload.length);
  const nextPayload = Buffer.from(payload);
  for (let index = 0; index + 1 < payload.length; index += 2) {
    const value = payload.readUInt16LE(index);
    const maxValue = index + 1 < maxPayload.length ? maxPayload.readUInt16LE(index) : 0;
    if (value > maxValue) {
      maxPayload.writeUInt16LE(value, index);
      nextPayload.writeUInt16LE(value, index);
    } else {
      nextPayload.writeUInt16LE(maxValue, index);
    }
  }
  state.calibration.maxPages.set(page, maxPayload);
  return nextPayload;
}

function venderLightPayload(light) {
  return Buffer.from([0x00, 0x0f, light.lightType === LIGHT_OTHER ? 0x00 : 0x01, 0x00]);
}

function hidTrace(report) {
  return (TRACE_HID_REPORTS ? report : report.subarray(0, 8)).toString("hex");
}

function featureIo(args) {
  if (!existsSync(FEATURE_IO)) {
    try {
      mkdirSync(dirname(FEATURE_IO), { recursive: true });
      execFileSync(process.env.CC ?? "cc", ["-Wall", "-Wextra", "-o", FEATURE_IO, FEATURE_IO_SOURCE], {
        stdio: "pipe",
      });
      console.log(`Built HID feature helper ${FEATURE_IO}`);
    } catch (error) {
      const compilerError = error.stderr?.toString("utf8").trim();
      const suffix = compilerError ? `: ${compilerError}` : "";
      throw new Error(`cannot build ${FEATURE_IO} from ${FEATURE_IO_SOURCE}${suffix}`);
    }
  }
  return execFileSync(FEATURE_IO, args, { encoding: "utf8" }).trim();
}

function detectedDeviceId() {
  if (process.env.MONSGEEK_DEVICE_ID) {
    return Number.parseInt(process.env.MONSGEEK_DEVICE_ID, 0);
  }

  const report = normalizedPayload(Buffer.from([GET_INFOR]), 0);
  featureIo(["send", DEFAULT_HIDRAW, report.toString("hex")]);
  const response = Buffer.from(featureIo(["read", DEFAULT_HIDRAW]), "hex");
  if (response[0] !== GET_INFOR || response.length < 3) {
    throw new Error(`GET_INFOR returned ${response.subarray(0, 8).toString("hex")}`);
  }
  return response.readUInt16LE(1);
}

function deviceListMessage() {
  const id = detectedDeviceId();
  const device = Buffer.concat([
    protobufString(3, DEFAULT_HIDRAW),
    protobufVarint(4, id),
    protobufVarint(6, 1),
    protobufVarint(7, 0x3151),
    protobufVarint(8, 0x502d),
  ]);
  const djDev = protobufBytes(1, device);
  return protobufBytes(1, djDev);
}

function resSend(err = "") {
  return err ? protobufString(1, err) : Buffer.alloc(0);
}

function resRead(payload, err = "") {
  const fields = [];
  if (err) {
    fields.push(protobufString(1, err));
  }
  if (payload.length > 0) {
    fields.push(protobufBytes(2, payload));
  }
  return Buffer.concat(fields);
}

function microphoneMuteStatus(err = "") {
  const fields = [protobufVarint(1, microphoneMuted ? 1 : 0)];
  if (err) {
    fields.push(protobufString(2, err));
  }
  return Buffer.concat(fields);
}

function syntheticWeatherPayload(request) {
  const city = decodeURIComponent(request.address || "").trim() || "Macau";
  const phrase = request.language.toLowerCase().startsWith("zh") ? "多云" : "Cloudy";
  return {
    errcode: "0",
    errmsg: "OK",
    city,
    day: {
      temperature: 25,
      phrase,
    },
  };
}

async function weatherResponse(requestMessage) {
  const request = decodeWeatherReq(requestMessage);
  if (LIVE_WEATHER) {
    try {
      const url = new URL(WEATHER_ENDPOINT);
      url.searchParams.set("version", "day");
      url.searchParams.set("unit", "m");
      url.searchParams.set("language", request.language || "en");
      url.searchParams.set("query", decodeURIComponent(request.address || "").trim() || "Macau");
      url.searchParams.set("appid", "48129135");
      url.searchParams.set("appsecret", "6Zojc6j0");
      const response = await fetch(url, { signal: AbortSignal.timeout(2500) });
      if (response.ok) {
        return protobufString(1, await response.text());
      }
    } catch {
      // Fall back to the offline-compatible payload below.
    }
  }
  return protobufString(1, JSON.stringify(syntheticWeatherPayload(request)));
}

function systemInfoMessage() {
  let diskSpaceTotal = 0;
  let diskSpceAvailable = 0;
  try {
    const disk = statfsSync("/");
    diskSpaceTotal = Number(disk.blocks) * Number(disk.bsize);
    diskSpceAvailable = Number(disk.bavail) * Number(disk.bsize);
  } catch {
    // Keep zero disk values when statfs is unavailable.
  }

  const memTotal = totalmem();
  const memFree = freemem();
  const memUsed = Math.max(0, memTotal - memFree);
  const cpuUsage = Math.max(0, Math.min(100, loadavg()[0] * 10));

  return Buffer.concat([
    protobufVarint(1, diskSpaceTotal),
    protobufVarint(2, diskSpceAvailable),
    protobufVarint(3, 0),
    protobufVarint(4, 0),
    protobufFloat(5, 0),
    protobufVarint(6, memTotal),
    protobufVarint(7, memUsed),
    protobufFloat(8, cpuUsage),
  ]);
}

function otaProgress(err, progress = 0) {
  const fields = [protobufFloat(1, progress)];
  if (err) {
    fields.push(protobufString(2, err));
  }
  return Buffer.concat(fields);
}

function dbItemNotFound() {
  return protobufString(2, "linux probe has no stored item");
}

function dbList(values) {
  return Buffer.concat(values.map((value) => protobufBytes(1, value)));
}

function loadDb() {
  if (!existsSync(DB_FILE)) {
    return { dbs: {} };
  }

  try {
    const db = JSON.parse(readFileSync(DB_FILE, "utf8"));
    return db && typeof db.dbs === "object" ? db : { dbs: {} };
  } catch (error) {
    throw new Error(`cannot read ${DB_FILE}: ${error.message}`);
  }
}

function saveDb(db) {
  mkdirSync(dirname(DB_FILE), { recursive: true });
  const tempFile = `${DB_FILE}.${process.pid}.tmp`;
  writeFileSync(tempFile, `${JSON.stringify(db, null, 2)}\n`);
  renameSync(tempFile, DB_FILE);
}

function dbBucket(db, dbpath) {
  if (!db.dbs[dbpath]) {
    db.dbs[dbpath] = {};
  }
  return db.dbs[dbpath];
}

function storedDbItem(requestMessage) {
  const request = decodeDbMessage(requestMessage);
  const db = loadDb();
  const value = db.dbs[request.dbpath]?.[request.key.toString("base64")];
  if (value === undefined) {
    return { message: dbItemNotFound(), note: `DB miss ${request.dbpath}` };
  }
  return {
    message: protobufBytes(1, Buffer.from(value, "base64")),
    note: `DB hit ${request.dbpath}`,
  };
}

function storedDbList(requestMessage, selector) {
  const request = decodeDbMessage(requestMessage);
  const entries = Object.entries(loadDb().dbs[request.dbpath] ?? {});
  return {
    message: dbList(entries.map(([key, value]) => Buffer.from(selector(key, value), "base64"))),
    note: `DB ${selector === dbKey ? "keys" : "values"} ${request.dbpath} count=${entries.length}`,
  };
}

function dbKey(key) {
  return key;
}

function dbValue(_key, value) {
  return value;
}

function storeDbItem(requestMessage) {
  const request = decodeDbMessage(requestMessage);
  const db = loadDb();
  dbBucket(db, request.dbpath)[request.key.toString("base64")] = request.value.toString("base64");
  saveDb(db);
  return { message: resSend(), note: `DB put ${request.dbpath}` };
}

function deleteDbItem(requestMessage) {
  const request = decodeDbMessage(requestMessage);
  const db = loadDb();
  const bucket = db.dbs[request.dbpath];
  if (bucket) {
    delete bucket[request.key.toString("base64")];
    saveDb(db);
  }
  return { message: resSend(), note: `DB delete ${request.dbpath}` };
}

function headers(contentType = "application/grpc-web-text") {
  return {
    "access-control-allow-credentials": "true",
    "access-control-allow-headers":
      "content-type,x-grpc-web,x-user-agent,grpc-timeout,authorization",
    "access-control-allow-methods": "POST,OPTIONS",
    "access-control-allow-private-network": "true",
    "access-control-allow-origin": WEB_ORIGIN,
    "access-control-expose-headers": "grpc-status,grpc-message,grpc-status-details-bin",
    "content-type": contentType,
    "x-content-type-options": "nosniff",
  };
}

async function responseFor(method, requestMessage) {
  if (method === "getVersion") {
    return {
      message: Buffer.concat([
        protobufString(1, "1.8.9-linux-probe"),
        protobufString(2, "20260522"),
      ]),
      note: "version response",
    };
  }

  if (method === "watchDevList") {
    return { message: deviceListMessage(), note: "MonsGeek device list" };
  }

  if (method === "watchVender") {
    return { message: Buffer.alloc(0), note: "empty stream event" };
  }

  if (method === "watchSystemInfo") {
    return { message: systemInfoMessage(), note: "Linux system info event" };
  }

  if (method === "changeWirelessLoopStatus") {
    return { message: Buffer.alloc(0), note: `${method} acknowledged` };
  }

  if (method === "setLightType") {
    const light = decodeSetLight(requestMessage);
    const state = stateFor(light.devicePath);
    state.light = light;
    broadcastStream("watchVender", venderMessage(venderLightPayload(light)));
    return {
      message: Buffer.alloc(0),
      note: `setLightType ${lightTypeName(light.lightType)} screen=${light.screenId} dangle=${light.dangleDevType}`,
    };
  }

  if (method === "muteMicrophone") {
    microphoneMuted = decodeBoolField(requestMessage, 1);
    return { message: resSend(), note: `microphone mute set ${microphoneMuted}` };
  }

  if (method === "toggleMicrophoneMute") {
    microphoneMuted = !microphoneMuted;
    return { message: resSend(), note: `microphone mute toggled ${microphoneMuted}` };
  }

  if (method === "getMicrophoneMute") {
    return { message: microphoneMuteStatus(), note: `microphone mute status ${microphoneMuted}` };
  }

  if (method === "getWeather") {
    return {
      message: await weatherResponse(requestMessage),
      note: LIVE_WEATHER ? "live/weather-fallback response" : "synthetic weather response",
    };
  }

  if (method === "upgradeOTAGATT") {
    const request = decodeOtaUpgrade(requestMessage);
    const err =
      process.env.MONSGEEK_ALLOW_OTA === "1"
        ? "Linux OTA transport is not implemented yet"
        : "Linux OTA is disabled; set MONSGEEK_ALLOW_OTA=1 only after implementing device-specific flashing";
    return {
      message: otaProgress(err),
      note: `OTA refused for ${request.devicePath || DEFAULT_HIDRAW} bytes=${request.fileBytes}`,
    };
  }

  if (method === "getItemFromDb") {
    return storedDbItem(requestMessage);
  }

  if (method === "getAllKeysFromDb") {
    return storedDbList(requestMessage, dbKey);
  }

  if (method === "getAllValuesFromDb") {
    return storedDbList(requestMessage, dbValue);
  }

  if (method === "insertDb") {
    return storeDbItem(requestMessage);
  }

  if (method === "deleteItemFromDb") {
    return deleteDbItem(requestMessage);
  }

  if (method === "cleanDev") {
    return { message: resSend(), note: "cleanDev acknowledged without persistence" };
  }

  if (method === "sendMsg" || method === "sendRawFeature") {
    const message = decodeSendMsg(requestMessage);
    try {
      const payload = normalizedPayload(message.payload, message.checksumType);
      featureIo(["send", message.devicePath || DEFAULT_HIDRAW, payload.toString("hex")]);
      updateWriteState(message, payload);
      return {
        message: resSend(),
        note: `${method} checksum=${message.checksumType} dangle=${message.dangleDevType} cmd=0x${payload[0].toString(16)} ${hidTrace(payload)}`,
      };
    } catch (error) {
      return { message: resSend(error.message), note: `${method} failed: ${error.message}` };
    }
  }

  if (method === "readMsg" || method === "readRawFeature") {
    const message = decodeReadMsg(requestMessage);
    try {
      const rawPayload = Buffer.from(featureIo(["read", message.devicePath || DEFAULT_HIDRAW]), "hex");
      const state = stateFor(message.devicePath);
      const payload = monotonicCalibrationPayload(state, rawPayload);
      return {
        message: resRead(payload),
        note: `${method} ${payload === rawPayload ? "" : "calibration-max "} ${hidTrace(payload)}`,
      };
    } catch (error) {
      return { message: resRead(Buffer.alloc(0), error.message), note: `${method} failed: ${error.message}` };
    }
  }

  return {
    message: Buffer.alloc(0),
    status: 12,
    statusMessage: "linux probe does not implement this method yet",
    note: "unimplemented method",
  };
}

function failedResponse(error) {
  const message = error instanceof Error ? error.message : String(error);
  return {
    message: Buffer.alloc(0),
    status: 13,
    statusMessage: message,
    note: `request failed: ${message}`,
  };
}

const server = http.createServer((request, response) => {
  if (request.method === "OPTIONS") {
    console.log(`OPTIONS ${request.url}`);
    response.writeHead(204, headers("text/plain"));
    response.end();
    return;
  }

  const body = [];
  request.on("data", (chunk) => body.push(chunk));
  request.on("end", async () => {
    const method = request.url?.split("/").at(-1) ?? "unknown";
    const bytes = Buffer.concat(body);
    const payload = requestPayload(bytes);
    if (method === "watchDevList") {
      console.log(`${request.method} ${request.url} ${payload} -> MonsGeek device list stream`);
      attachStream(method, response, deviceListMessage, 5000);
      return;
    }
    if (method === "watchVender") {
      console.log(`${request.method} ${request.url} ${payload} -> vendor event stream`);
      startVendorInputReader(DEFAULT_HIDRAW);
      attachStream(method, response, () => {
        const firstState = deviceStates.values().next().value;
        return venderMessage(firstState ? venderLightPayload(firstState.light) : Buffer.alloc(0));
      }, 5000);
      return;
    }
    if (method === "watchSystemInfo") {
      console.log(`${request.method} ${request.url} ${payload} -> Linux system info stream`);
      attachStream(method, response, systemInfoMessage, 5000);
      return;
    }
    let result;
    try {
      result = await responseFor(method, decodeGrpcTextBody(bytes));
    } catch (error) {
      result = failedResponse(error);
    }
    console.log(`${request.method} ${request.url} ${payload} -> ${result.note}`);
    response.writeHead(200, headers());
    response.end(grpcTextResponse(result.message, result.status, result.statusMessage));
  });
});

server.listen(PORT, HOST, () => {
  console.log(`MonsGeek gRPC-Web probe listening on http://${HOST}:${PORT}`);
  console.log(`Using HID endpoint ${DEFAULT_HIDRAW}`);
  console.log(`Using DB file ${DB_FILE}`);
  console.log(`Open ${WEB_ORIGIN} and watch which /driver.DriverGrpc calls arrive.`);
});

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    stopVendorInputReaders();
    process.exit(signal === "SIGINT" ? 130 : 143);
  });
}
