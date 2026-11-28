// The installer page: the steps, the buttons, texts and theme. The work is in omap.js (the
// boot ROM), service.js (backup, boot loader, restore) and dfu.js (the system).
import { romBoot, ROM_FILTER } from "./omap.js";
import { Dfu, DFU_FILTER } from "./dfu.js";
import { Service, ByteReader, readBackup, verifyBackup, notAJ49, NEVER_WRITTEN } from "./service.js";
import { UsbSerial, ACM_FILTER } from "./usbserial.js";
// ?webusb in the address: use WebUSB for the serial port also where Web Serial exists (to test).
// Chrome on Android has Web Serial, but only for Bluetooth: its list never shows a USB port.
const FORCE_WEBUSB = new URLSearchParams(location.search).has("webusb") || /Android/.test(navigator.userAgent);

const $ = id => document.getElementById(id);
// The languages, each with its flag as a small drawing (flag emoji do not show on Windows).
// To add one: a file lang/<code>.txt and an entry here.
const LANGS = {
  en: '<svg viewBox="0 0 60 40"><rect width="60" height="40" fill="#012169"/><path d="M0 0L60 40M60 0L0 40" stroke="#fff" stroke-width="8"/><path d="M0 0L60 40M60 0L0 40" stroke="#c8102e" stroke-width="3"/><path d="M30 0V40M0 20H60" stroke="#fff" stroke-width="12"/><path d="M30 0V40M0 20H60" stroke="#c8102e" stroke-width="7"/></svg>',
  nl: '<svg viewBox="0 0 60 40"><rect width="60" height="40" fill="#fff"/><rect width="60" height="13.4" fill="#ae1c28"/><rect y="26.6" width="60" height="13.4" fill="#21468b"/></svg>',
};
// The USB serial gadget of the rescue kernel (Linux g_serial).
const SERIAL_FILTER = { usbVendorId: 0x0525, usbProductId: 0xa4a7 };
// Where the loaders go in RAM (tools/dfu-install/session1-stock.sh).
const ADDR_UBOOT = 0x80100000, ADDR_KERNEL = 0x80a00000;

// What each mode needs. "sha256" comes from images/manifest.json when the page has one.
const ROLES = {
  xload: { text: "files.xload" }, ubootram: { text: "files.ubootram" }, service: { text: "files.service" },
  uboot: { text: "files.uboot" }, rootfs: { text: "files.rootfs" }, kernel: { text: "files.kernel" },
  backupfile: { text: "files.backupfile", local: true },
};
const NEEDS = {
  install: ["xload", "ubootram", "service", "uboot", "rootfs", "kernel"],
  backup: ["xload", "ubootram", "service"],
  restore: ["xload", "ubootram", "service", "backupfile"],
};
const STEPS = {
  install: ["files", "rom", "svc", "backup", "uboot", "dfu", "sys", "done"],
  backup: ["files", "rom", "svc", "backup", "done"],
  restore: ["files", "rom", "svc", "restore", "done"],
};

let texts = {}, fallback = {}, mode = null, step = 0, manifest = null;
const files = {};        // role -> {name, data: Uint8Array | file: File, sum}
let service = null, port = null, serialIo = null;

// ---------- texts and theme ----------
const store = { get(k) { try { return localStorage.getItem(k); } catch (e) { return null; } }, set(k, v) { try { localStorage.setItem(k, v); } catch (e) { /* private window */ } } };
const parse = text => Object.fromEntries(text.split("\n").filter(l => l && !l.startsWith("#") && l.includes("=")).map(l => [l.slice(0, l.indexOf("=")).trim(), l.slice(l.indexOf("=") + 1).trim()]));
const T = (k, vars) => { let s = texts[k] || fallback[k] || k; for (const [n, v] of Object.entries(vars || {})) s = s.replace("{" + n + "}", v); return s; };
// Nothing the page loads may come from the browser's cache: a rebuilt image with an old copy
// of the list of checksums next to it passes every check and installs the old image. (Seen on
// 27 November 2026: a kernel of three hours before was written to the thermostat.)
const FRESH = { cache: "no-store" };
async function getText(url) { const r = await fetch(url, FRESH); if (!r.ok) throw new Error(url + ": " + r.status); return r.text(); }
async function setLang(code) {
  if (!(code in LANGS)) code = "en";
  try { texts = parse(await getText("lang/" + code + ".txt")); } catch (e) { texts = {}; code = "en"; }
  store.set("j49.lang", code); document.documentElement.lang = code;
  document.querySelectorAll("#flags button").forEach(b => b.setAttribute("aria-pressed", b.dataset.lang === code));
  document.title = T("title");
  document.querySelectorAll("[data-t]").forEach(e => e.textContent = T(e.dataset.t));
  $("flags").setAttribute("aria-label", T("language"));
  if (mode) paintFiles();
}
// ---------- small helpers ----------
function log(line) { const p = $("log"); p.textContent += line + "\n"; p.scrollTop = p.scrollHeight; }
function state(id, text, kind) { const e = $(id + "-state"); e.textContent = text; e.className = "state " + (kind || ""); }
function bar(id, done, total) { $(id + "-bar").style.width = (total ? Math.min(100, done * 100 / total) : 0) + "%"; }
const mb = n => (n / 1048576).toFixed(1);
const hex = buf => Array.from(new Uint8Array(buf), b => b.toString(16).padStart(2, "0")).join("");
function show() {
  const order = STEPS[mode];
  for (const s of ["files", "rom", "svc", "backup", "uboot", "dfu", "sys", "restore", "done"]) {
    const el = $("s-" + s), i = order.indexOf(s);
    el.hidden = i < 0;
    el.className = i < step ? "done" : i === step ? "active" : "later";
  }
  $("done-text").textContent = T("done." + mode);
}
// A step can be started by hand with a click on its title: for a thermostat that is already
// further along (in the installer mode of U-Boot, for example).
for (const sname of ["rom", "svc", "backup", "uboot", "dfu", "sys", "restore"]) {
  $("s-" + sname).querySelector("h2").addEventListener("click", () => {
    if (!mode) return;
    const i = STEPS[mode].indexOf(sname);
    if (i >= 0) { step = i; show(); }
  });
}
function next() { step++; show(); const s = STEPS[mode][step]; if (s) $("s-" + s).scrollIntoView({ behavior: "smooth", block: "center" }); }
/** Run one step's work behind its button: errors go to the step's state line and the log. */
function guard(id, button, work) {
  button.onclick = async () => {
    button.disabled = true;
    try { await work(); }
    catch (e) {
      const cancelled = e && (e.name === "NotFoundError" || e.name === "AbortError");
      const text = e && e.code && texts["error." + e.code] || fallback["error." + (e && e.code)] || (e && e.message || e);
      state(id, cancelled ? T("cancelled") : T("error") + ": " + text, cancelled ? "" : "bad");
      if (!cancelled) log("ERROR " + (e && e.stack || e));
      button.disabled = false;
    }
  };
}

// ---------- step 1: files ----------
/** The text of the release whose images this page installs, from GitHub, as plain elements
 *  (nothing of it is used as HTML). images/release.json names it: {"repo": "owner/name", "tag": "..."}. */
async function showRelease() {
  let rel;
  try { rel = JSON.parse(await getText("images/release.json")); } catch (e) { return; }
  if (!rel || !/^[\w.-]+\/[\w.-]+$/.test(rel.repo || "") || !/^[\w.+-]+$/.test(rel.tag || "")) return;
  let r;
  try {
    const res = await fetch("https://api.github.com/repos/" + rel.repo + "/releases/tags/" + rel.tag, FRESH);
    if (!res.ok) return;
    r = await res.json();
  } catch (e) { return; }
  const plain = t => t.replace(/\*\*|`/g, "").replace(/\[([^\]]+)\]\([^)]+\)/g, "$1");
  const body = $("rel-body"); body.textContent = "";
  let list = null, pre = null;
  for (const raw of String(r.body || "").split(/\r?\n/)) {
    const line = raw.trimEnd();
    const add = (tag, text) => { const e = document.createElement(tag); e.textContent = plain(text); return e; };
    if (line.startsWith("|")) {                       // a table: as it is, in a fixed-width block
      if (/^\|[\s:|-]+\|$/.test(line)) continue;
      if (!pre) { pre = document.createElement("pre"); body.append(pre); }
      pre.textContent += plain(line.replace(/^\||\|$/g, "").replace(/\|/g, "   ")) + "\n"; list = null; continue;
    }
    pre = null;
    if (/^#+ /.test(line)) { body.append(add("h3", line.replace(/^#+ /, ""))); list = null; }
    else if (/^[-*] /.test(line)) { if (!list) { list = document.createElement("ul"); body.append(list); } list.append(add("li", line.slice(2))); }
    else if (line.trim()) { body.append(add("p", line)); list = null; }
    else list = null;
  }
  $("rel-name").textContent = (r.name || rel.tag) + (r.prerelease ? " (pre-release)" : "");
  const url = String(r.html_url || "");
  if (url.startsWith("https://github.com/")) $("rel-link").href = url; else $("rel-link").hidden = true;
  $("s-release").hidden = false;
}
async function loadManifest() {
  try { manifest = JSON.parse(await getText("images/manifest.json")); } catch (e) { manifest = null; }
}
async function checkFile(role, f) {
  const want = manifest && manifest[role] && manifest[role].sha256;
  if (ROLES[role].local) { f.sum = "ok"; return; }
  const sum = hex(await crypto.subtle.digest("SHA-256", f.data));
  f.sha256 = sum;
  f.sum = !want ? "unknown" : sum === want ? "ok" : "bad";
}
function paintFiles() {
  const box = $("files"); box.textContent = "";
  $("files-how").textContent = T(manifest ? "files.auto" : "files.manual");
  let ready = true;
  for (const role of NEEDS[mode]) {
    const row = document.createElement("div"); row.className = "frow";
    const name = document.createElement("label"); name.textContent = T(ROLES[role].text); name.htmlFor = "f-" + role;
    const input = document.createElement("input"); input.type = "file"; input.id = "f-" + role;
    input.onchange = async () => {
      const file = input.files[0]; if (!file) return;
      files[role] = ROLES[role].local ? { name: file.name, file } : { name: file.name, data: new Uint8Array(await file.arrayBuffer()) };
      await checkFile(role, files[role]); paintFiles();
    };
    const st = document.createElement("div"); st.className = "st";
    const f = files[role];
    if (f) {
      const size = f.data ? f.data.length : f.file.size;
      st.textContent = f.name + ", " + mb(size) + " MB: " + (f.sum === "ok" ? T("files.ok") : f.sum === "bad" ? T("files.badsum") : T("files.unknown") + (f.sha256 ? " sha256 " + f.sha256.slice(0, 16) + "…" : ""));
      st.className = "st " + (f.sum === "ok" ? "ok" : f.sum === "bad" ? "bad" : "");
      if (f.sum === "bad") ready = false;
    } else ready = false;
    row.append(name, input, st); box.appendChild(row);
  }
  state("files", ready ? "" : T("files.missing"));
  if (ready && STEPS[mode][step] === "files") next();
}
async function fetchImages() {
  if (!manifest) return;
  for (const role of NEEDS[mode]) {
    const m = manifest[role];
    if (!m || files[role]) continue;
    try {
      const r = await fetch("images/" + m.file, FRESH);
      if (!r.ok) continue;
      files[role] = { name: m.file, data: new Uint8Array(await r.arrayBuffer()) };
      await checkFile(role, files[role]);
    } catch (e) { /* the user can choose the file */ }
    paintFiles();
  }
}

// ---------- step 2: the boot ROM ----------
async function runRom() {
  state("rom", "");
  const dev = await navigator.usb.requestDevice({ filters: [ROM_FILTER] });
  state("rom", T("rom.sending"));
  await romBoot(dev, [
    { name: "x-load", data: files.xload.data },
    { name: "u-boot", data: files.ubootram.data, addr: ADDR_UBOOT },
    { name: "kernel", data: files.service.data, addr: ADDR_KERNEL },
  ], ADDR_UBOOT, { log, progress: (d, t) => bar("rom", d, t) });
  state("rom", T("rom.done"), "ok");
  next();
}

// ---------- step 3: the service ----------
/** Let go of the serial port: stop reading, give the streams back, close. Without this the
 *  browser keeps a port whose device has left the bus. */
async function closePort() {
  if (!port) return;
  if (port instanceof UsbSerial) { await port.close(); port = null; return; }
  try { await serialIo.rd.cancel(); } catch (e) { /* already ended */ }
  try { serialIo.rd.releaseLock(); serialIo.wr.releaseLock(); } catch (e) { /* already released */ }
  try { await port.close(); } catch (e) { /* the device is gone */ }
  port = null;
}
/** Open the serial port of the thermostat and get in step with nandio on it. `id` is the
 *  step whose state line shows the waiting. */
async function openService(id) {
  state(id, "");
  // a port left open by an earlier try is closed first
  await closePort();
  let reader, write;
  if (navigator.serial && !FORCE_WEBUSB) {
    port = await navigator.serial.requestPort({ filters: [SERIAL_FILTER] });
    await port.open({ baudRate: 115200, bufferSize: 1 << 20 });
    const rd = port.readable.getReader(), wr = port.writable.getWriter();
    serialIo = { rd, wr };
    reader = new ByteReader(async () => { const { value, done } = await rd.read(); return done ? null : value; });
    write = d => wr.write(d);
  } else {
    // no Web Serial (Chrome on Android): the same port through WebUSB
    const us = new UsbSerial(await navigator.usb.requestDevice({ filters: [ACM_FILTER] }));
    await us.open();
    port = us;
    log("serial port through WebUSB" + (us.note ? " (" + us.note + ")" : ""));
    reader = new ByteReader(() => us.pull());
    write = d => us.write(d);
  }
  const svc = new Service(reader, write, log);
  state(id, T("svc.waiting"));
  // count what comes in, so a silent line and a line that only repeats itself can be told apart
  let got = 0; const pull0 = reader.pull;
  reader.pull = async () => { const v = await pull0(); if (v) { got += v.length; if (!svc.parts) state(id, T("svc.waiting") + " (" + got + " bytes)"); } return v; };
  await svc.start();
  await svc.info();
  return svc;
}
async function runService() {
  service = await openService("svc");
  const parts = service.parts;
  // Is it the thermostat this installer is for? Nothing is read or written before this.
  const ident = await service.ident();
  log("the board says: model " + ident.model + ", hardware " + ident.hardware + ", revision " + ident.revision);
  const why = notAJ49(parts, ident);
  if (why) { log("not a J49: " + why); service = null; const e = new Error(why); e.code = "wrongdevice"; throw e; }
  await service.show(T("panel.connected"), 0, 0, "");
  const chip = parts.filter(p => p.name !== "nand0").reduce((n, p) => n + p.size, 0);
  log("partitions: " + parts.map(p => p.name).join(" "));
  state("svc", T("svc.found", { n: parts.filter(p => p.name !== "nand0").length, mb: mb(chip) }), "ok");
  next();
}

// ---------- step 4: backup ----------
async function runBackup() {
  const name = "nest-" + new Date().toISOString().slice(0, 10) + ".j49backup";
  let sink, close, fileBack;
  if (window.showSaveFilePicker) {
    const handle = await window.showSaveFilePicker({ suggestedName: name });
    const w = await handle.createWritable();
    sink = b => w.write(b); close = () => w.close(); fileBack = () => handle.getFile();
  } else {                                    // no file picker: keep it in memory, download at the end
    const pieces = [];
    sink = async b => { pieces.push(b.slice()); };
    close = async () => { const a = document.createElement("a"); a.href = URL.createObjectURL(new Blob(pieces)); a.download = name; a.click(); };
    fileBack = async () => new Blob(pieces);
  }
  const summary = await service.backup(sink, (d, t, n) => { bar("backup", d, t); state("backup", T("backup.reading", { name: n })); }, { date: new Date().toISOString() }, T("panel.backup"));
  await service.show(T("panel.backup"), 100, 0, T("panel.checking"));
  await close();
  state("backup", T("backup.verify"));
  const file = await fileBack(), stream = file.stream().getReader();
  await verifyBackup(new ByteReader(async () => { const { value, done } = await stream.read(); return done ? null : value; }));
  const errors = summary.reduce((n, p) => n + p.X, 0);
  state("backup", T("backup.done", { mb: mb(file.size) }) + (errors ? " " + T("backup.errors", { n: errors }) : ""), errors ? "bad" : "ok");
  log("backup: " + file.size + " bytes, " + errors + " blocks with read errors");
  await service.show(errors ? "ERROR" : "DONE", 100, 0, T("panel.backup"));
  if (mode === "backup") { await service.reboot(); await closePort(); }
  next();
}

// ---------- step 5: the boot loader ----------
const NVS_STOCK = "wl12xx-nvs.bin";
const NVS_J49 = "j49-persist/override/lib/firmware/ti-connectivity/wl127x-nvs.bin";
async function runUboot() {
  for (const name of ["spl0", "spl1"]) {
    state("uboot", T("uboot.writing", { name }));
    await service.show(T("panel.bootloader"), name === "spl1" ? 50 : 0, 50, name);
    await service.writeImage(name, files.uboot.data, (d, t) => bar("uboot", (name === "spl1" ? t : 0) + d, 2 * t));
    log(name + " written and read back");
  }
  // The kernel we install starts the system in root0 when there is one, and waits for the
  // installer when there is none. Erase root0, so that it waits whatever was in it (the stock
  // system, or ours from an earlier install).
  state("uboot", T("uboot.writing", { name: "root0" }));
  await service.show(T("panel.bootloader"), 0, 100, "root0");
  await service.erase("root0");
  log("root0 erased");
  // Our kernel cannot read what the stock firmware wrote (another error correction layout),
  // and the reverse. Two things follow, and both must happen now, under the stock kernel:
  // - the calibration of the WiFi chip (with its MAC address) is fetched from the stock
  //   partition system-config; step 7 gives it to the new system. It is kept in this browser
  //   until then, for the case that the page is loaded again in between.
  // - the partition scratch, which the new system uses for what must survive a reinstall, is
  //   erased when stock data is in it. When the stock kernel cannot read it, it holds the data
  //   of an earlier J49 install (WiFi network, keys): that stays.
  if (service.parts.find(p => p.name === "system-config")) {
    const nvs = await service.getFile("system-config", NVS_STOCK);
    if (nvs && nvs.length === 912) { store.set("j49.nvs", btoa(String.fromCharCode(...nvs))); log("WiFi calibration of this thermostat fetched (" + nvs.length + " bytes)"); }
    else log("no WiFi calibration found in system-config: the thermostat will have a random WiFi address");
  }
  if (service.parts.find(p => p.name === "scratch")) {
    const c = await service.check("scratch");
    if (c.failed === 0) {
      state("uboot", T("uboot.writing", { name: "scratch" }));
      await service.show(T("panel.bootloader"), 0, 100, "scratch");
      await service.erase("scratch");
      log("scratch erased (it held stock data, which the new kernel cannot read)");
    } else log("scratch holds data of an earlier J49 install: kept");
  }
  await service.show("DONE", 100, 0, T("panel.bootloader"));
  // Our U-Boot starts the kernel in the flash memory when it can, and waits for a new kernel
  // only when told to: the service leaves a mark in RAM for it.
  const how = await service.reboot("dfu");
  log("restart into the installer mode of U-Boot: " + (how || "no answer"));
  if (how === "OK nomark") throw new Error("could not tell U-Boot to wait for the installer");
  await closePort();
  state("uboot", T("uboot.done"), "ok");
  next();
}

// ---------- step 6: the system, over DFU ----------
async function dfuDevice(ask) {
  const known = (await navigator.usb.getDevices()).find(d => d.vendorId === DFU_FILTER.vendorId && d.productId === DFU_FILTER.productId);
  if (known) return known;
  return ask ? navigator.usb.requestDevice({ filters: [DFU_FILTER] }) : null;
}
async function runDfu() {
  const dfu = new Dfu(await dfuDevice(true), log);
  log("DFU targets: " + (await dfu.open()).join(", "));
  // The kernel goes into the flash memory (boot0), written by U-Boot so that U-Boot can read
  // it. When we leave, the thermostat restarts and starts that kernel; root0 is empty, so the
  // kernel waits for the installer (the next step). Nothing is started from RAM: that was
  // tried and our U-Boot restarts the board instead.
  state("dfu", T("dfu.sending", { name: "boot0" }));
  await dfu.download("boot0", files.kernel.data, (d, t) => bar("dfu", d, t));
  await dfu.detach();
  state("dfu", T("dfu.sent"), "ok");
  next();
  // The thermostat now restarts and is four USB devices one after the other (the boot ROM,
  // U-Boot with a serial port of its own, then the kernel). With the list of serial ports open
  // during that, Chrome crashed. So the next step waits until the kernel is up.
  const go = $("sys-go");
  go.disabled = true;
  for (let n = 25; n > 0; n--) { state("sys", T("sys.soon", { n })); await new Promise(r => setTimeout(r, 1000)); }
  state("sys", "");
  go.disabled = false; go.focus();
}

// ---------- step 7: the root file system, through nandio in the installer kernel ----------
async function runSys() {
  const svc = await openService("sys");
  const root = svc.parts.find(p => p.name === "root0");
  if (!root) throw new Error("the installer kernel shows no partition root0");
  if (files.rootfs.data.length > root.size) throw new Error("the root file system is larger than root0");
  log("installer kernel: " + svc.parts.map(p => p.name).join(" "));
  state("sys", T("sys.writing"));
  await svc.show(T("panel.system"), 0, 100, "root0");
  const r = await svc.writeImage("root0", files.rootfs.data, (d, t) => bar("sys", d, t));
  log("root0 written and read back" + (r.skipped ? " (" + r.skipped + " bad blocks left out)" : ""));
  // the WiFi calibration fetched in step 5, to where the new system looks for it
  const nvs = store.get("j49.nvs");
  if (nvs && svc.parts.find(p => p.name === "scratch")) {
    await svc.putFile("scratch", NVS_J49, Uint8Array.from(atob(nvs), c => c.charCodeAt(0)));
    log("WiFi calibration stored for the new system");
  }
  await svc.show("DONE", 100, 0, T("panel.system"));
  await svc.reboot();
  await closePort();
  state("sys", T("sys.done"), "ok");
  next();
}

// ---------- restore ----------
async function runRestore() {
  const open = () => { const s = files.backupfile.file.stream().getReader(); return new ByteReader(async () => { const { value, done } = await s.read(); return done ? null : value; }); };
  state("restore", T("restore.check"));
  const listed = await verifyBackup(open());
  const mine = new Map(service.parts.map(p => [p.name, p.size]));
  if (!listed.every(p => mine.get(p.name) === p.size)) throw new Error(T("restore.other"));
  const todo = listed.filter(p => !NEVER_WRITTEN.includes(p.name)), total = todo.reduce((n, p) => n + p.size, 0);
  let done = 0;
  for await (const p of readBackup(open())) {
    if (!p.name || NEVER_WRITTEN.includes(p.name)) continue;
    state("restore", T("restore.writing", { name: p.name }));
    await service.show(T("panel.restore"), done * 100 / total, (done + p.size) * 100 / total - Math.floor(done * 100 / total), p.name);
    const r = await service.writeFrames(p.name, p.frames, "exact", (d, t) => bar("restore", done + p.size * d / t, total));
    done += p.size;
    log(p.name + " written back" + (r.skipped ? " (" + r.skipped + " bad blocks left out)" : ""));
  }
  await service.show("DONE", 100, 0, T("panel.restore"));
  await service.reboot();
  await closePort();
  state("restore", T("restore.done"), "ok");
  next();
}

// ---------- start ----------
for (const [code, flag] of Object.entries(LANGS)) {
  const b = document.createElement("button");
  b.dataset.lang = code; b.innerHTML = flag; b.onclick = () => setLang(code);
  $("flags").appendChild(b);
}
try { fallback = parse(await getText("lang/en.txt")); } catch (e) { fallback = {}; }
await setLang(store.get("j49.lang") || ((navigator.language || "en").slice(0, 2)));
// the name of each language, in that language, for screen readers and as a tooltip
for (const b of document.querySelectorAll("#flags button")) {
  try { const t = parse(await getText("lang/" + b.dataset.lang + ".txt")); b.title = t["lang.name"] || b.dataset.lang; b.setAttribute("aria-label", b.title); } catch (e) { b.title = b.dataset.lang; }
}

const usable = !!navigator.usb;
$("nobrowser").hidden = usable;
$("insecure").hidden = window.isSecureContext;
$("windows").hidden = !/Windows/.test(navigator.userAgent);
await loadManifest();
$("images-note").hidden = !manifest;
showRelease();
const start = () => { const ok = usable && window.isSecureContext && $("agree").checked; document.querySelectorAll("[data-mode]").forEach(b => b.disabled = !ok); };
$("agree").onchange = start; start();
document.querySelectorAll("[data-mode]").forEach(b => b.onclick = () => {
  mode = b.dataset.mode; step = 0;
  document.querySelectorAll("[data-mode]").forEach(x => x.classList.toggle("on", x === b));
  $("steps").hidden = false; show(); paintFiles(); fetchImages();
});
guard("rom", $("rom-go"), runRom);
guard("svc", $("svc-go"), runService);
guard("backup", $("backup-go"), runBackup);
$("backup-skip").onclick = () => { if (mode === "install") next(); };
guard("uboot", $("uboot-go"), runUboot);
guard("dfu", $("dfu-go"), runDfu);
guard("sys", $("sys-go"), runSys);
guard("restore", $("restore-go"), runRestore);
