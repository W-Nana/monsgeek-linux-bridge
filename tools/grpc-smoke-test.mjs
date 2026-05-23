#!/usr/bin/env node

import http from "node:http";

const HOST = process.env.MONSGEEK_BRIDGE_HOST ?? "127.0.0.1";
const PORT = Number.parseInt(process.env.MONSGEEK_BRIDGE_PORT ?? "3814", 10);
const ORIGIN = "https://web.monsgeek.com";
const STREAMING_METHODS = new Set(["watchDevList", "watchVender", "watchSystemInfo"]);

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

function protobufBool(field, value) {
  return Buffer.concat([encodeVarint(field << 3), encodeVarint(value ? 1 : 0)]);
}

function protobufBytes(field, value) {
  return Buffer.concat([encodeVarint((field << 3) | 2), encodeVarint(value.length), value]);
}

function grpcRequest(payload = Buffer.alloc(0)) {
  const header = Buffer.alloc(5);
  header.writeUInt32BE(payload.length, 1);
  return Buffer.concat([header, payload]).toString("base64");
}

function parseGrpcText(text) {
  const raw = Buffer.from(text, "base64");
  const frames = [];
  for (let index = 0; index + 5 <= raw.length; ) {
    const flag = raw[index];
    const length = raw.readUInt32BE(index + 1);
    const payload = raw.subarray(index + 5, index + 5 + length);
    frames.push({ flag, payload });
    index += 5 + length;
  }
  return frames;
}

function grpcStatus(frames) {
  const trailer = frames.find((frame) => frame.flag === 0x80)?.payload.toString("ascii") ?? "";
  const match = trailer.match(/grpc-status:(\d+)/);
  return match ? Number.parseInt(match[1], 10) : undefined;
}

function call(method, payload = Buffer.alloc(0)) {
  return new Promise((resolve, reject) => {
    let settled = false;
    const settle = (value) => {
      if (!settled) {
        settled = true;
        resolve(value);
      }
    };
    const body = grpcRequest(payload);
    const request = http.request(
      {
        hostname: HOST,
        port: PORT,
        path: `/driver.DriverGrpc/${method}`,
        method: "POST",
        headers: {
          "content-type": "application/grpc-web-text",
          "content-length": Buffer.byteLength(body),
          "origin": ORIGIN,
          "x-grpc-web": "1",
        },
      },
      (response) => {
        const chunks = [];
        response.on("data", (chunk) => {
          chunks.push(chunk);
          if (STREAMING_METHODS.has(method)) {
            const frames = parseGrpcText(Buffer.concat(chunks).toString("ascii"));
            const firstMessage = frames.find((frame) => frame.flag === 0x00);
            if (firstMessage) {
              settle({
                method,
                httpStatus: response.statusCode,
                grpcStatus: 0,
                responseBytes: firstMessage.payload.length,
              });
              request.destroy();
            }
          }
        });
        response.on("end", () => {
          const frames = parseGrpcText(Buffer.concat(chunks).toString("ascii"));
          settle({
            method,
            httpStatus: response.statusCode,
            grpcStatus: grpcStatus(frames),
            responseBytes: frames.find((frame) => frame.flag === 0x00)?.payload.length ?? 0,
          });
        });
      },
    );
    request.setTimeout(3000, () => {
      request.destroy(new Error(`${method} timed out`));
    });
    request.on("error", (error) => {
      if (settled && STREAMING_METHODS.has(method)) {
        return;
      }
      reject(error);
    });
    request.end(body);
  });
}

const empty = Buffer.alloc(0);
const readMsg = protobufString(1, process.env.MONSGEEK_HIDRAW ?? "/dev/hidraw4");
const sendMsg = Buffer.concat([readMsg, protobufBytes(2, Buffer.from([0x8f])), protobufBool(3, false)]);
const dbPath = "web_driver/iot_db/smoke";
const dbKey = Buffer.from("smoke-key");
const dbValue = Buffer.from("smoke-value");

const tests = [
  ["getVersion", empty],
  ["watchDevList", empty],
  ["watchVender", empty],
  ["watchSystemInfo", empty],
  ["sendMsg", sendMsg],
  ["readMsg", readMsg],
  ["sendRawFeature", sendMsg],
  ["readRawFeature", readMsg],
  ["setLightType", Buffer.concat([readMsg, encodeVarint((2 << 3) | 0), encodeVarint(0)])],
  ["muteMicrophone", protobufBool(1, true)],
  ["toggleMicrophoneMute", empty],
  ["getMicrophoneMute", empty],
  ["changeWirelessLoopStatus", protobufBool(1, false)],
  ["insertDb", Buffer.concat([protobufString(1, dbPath), protobufBytes(2, dbKey), protobufBytes(3, dbValue)])],
  ["getItemFromDb", Buffer.concat([protobufString(1, dbPath), protobufBytes(2, dbKey)])],
  ["getAllKeysFromDb", protobufString(1, dbPath)],
  ["getAllValuesFromDb", protobufString(1, dbPath)],
  ["deleteItemFromDb", Buffer.concat([protobufString(1, dbPath), protobufBytes(2, dbKey)])],
  ["cleanDev", readMsg],
  ["getWeather", Buffer.concat([protobufString(1, "en"), protobufString(2, encodeURIComponent("Macau"))])],
  ["upgradeOTAGATT", Buffer.concat([readMsg, protobufBytes(2, Buffer.from([1, 2, 3, 4]))])],
];

let failed = false;
for (const [method, payload] of tests) {
  const result = await call(method, payload);
  const ok = result.httpStatus === 200 && result.grpcStatus === 0;
  failed ||= !ok;
  console.log(
    `${ok ? "ok" : "FAIL"} ${method} http=${result.httpStatus} grpc=${result.grpcStatus} bytes=${result.responseBytes}`,
  );
}

if (failed) {
  process.exitCode = 1;
}
