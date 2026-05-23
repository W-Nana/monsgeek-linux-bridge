#!/usr/bin/env node

import { readFile, readdir, realpath } from "node:fs/promises";
import path from "node:path";

const HIDRAW_ROOT = "/sys/class/hidraw";

async function readText(file) {
  try {
    return (await readFile(file, "utf8")).trim();
  } catch {
    return undefined;
  }
}

async function readBytes(file) {
  try {
    return await readFile(file);
  } catch {
    return undefined;
  }
}

async function findUsbDevice(start) {
  let current = start;

  for (;;) {
    const vendor = await readText(path.join(current, "idVendor"));
    const product = await readText(path.join(current, "idProduct"));
    if (vendor && product) {
      return {
        path: current,
        vendor,
        product,
        name: await readText(path.join(current, "product")),
        maker: await readText(path.join(current, "manufacturer")),
        revision: await readText(path.join(current, "bcdDevice")),
      };
    }

    const parent = path.dirname(current);
    if (parent === current || parent === "/sys") {
      return undefined;
    }
    current = parent;
  }
}

function formatBytes(bytes) {
  if (!bytes) {
    return "unreadable";
  }

  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join(" ");
}

function inspectFeatureDescriptor(bytes) {
  if (!bytes) {
    return [];
  }

  const facts = [];
  for (let index = 0; index < bytes.length; ) {
    const item = bytes[index];
    if (item === 0xfe) {
      index += (bytes[index + 1] ?? 0) + 3;
      continue;
    }

    const sizeCode = item & 0x03;
    const size = sizeCode === 3 ? 4 : sizeCode;
    const value = bytes[index + 1];
    if (item === 0x05) {
      facts.push(`usage_page=0x${value.toString(16)}`);
    }
    if (item === 0x06 && index + 2 < bytes.length) {
      const page = value | (bytes[index + 2] << 8);
      facts.push(`usage_page=0x${page.toString(16)}`);
    }
    if (item === 0x95) {
      facts.push(`report_count=${value}`);
    }
    if (item === 0x75) {
      facts.push(`report_size=${value}`);
    }
    if (item === 0xb1) {
      facts.push("feature_report");
    }
    index += size + 1;
  }
  return facts;
}

const hidrawNames = (await readdir(HIDRAW_ROOT))
  .filter((name) => name.startsWith("hidraw"))
  .sort((left, right) => Number(left.slice(6)) - Number(right.slice(6)));

for (const name of hidrawNames) {
  const hidPath = await realpath(path.join(HIDRAW_ROOT, name, "device"));
  const reportDescriptor = await readBytes(path.join(HIDRAW_ROOT, name, "device", "report_descriptor"));
  const uevent = await readText(path.join(HIDRAW_ROOT, name, "device", "uevent"));
  const usb = await findUsbDevice(hidPath);
  const isMonsGeek = usb?.vendor.toLowerCase() === "3151" || usb?.name?.includes("MonsGeek");

  console.log(`${isMonsGeek ? "*" : "-"} ${name}`);
  console.log(`  hid: ${hidPath}`);
  if (usb) {
    console.log(`  usb: ${usb.vendor}:${usb.product} ${usb.name ?? ""}`.trimEnd());
    console.log(`  maker: ${usb.maker ?? "(blank)"} revision=${usb.revision ?? "unknown"}`);
  }
  if (uevent) {
    console.log(`  ${uevent.split("\n").find((line) => line.startsWith("HID_ID=")) ?? "HID_ID=unknown"}`);
  }
  console.log(`  descriptor: ${formatBytes(reportDescriptor)}`);
  const facts = inspectFeatureDescriptor(reportDescriptor);
  if (facts.length > 0) {
    console.log(`  facts: ${facts.join(" ")}`);
  }
}
