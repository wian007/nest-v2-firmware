// The installer page: the steps, the buttons, texts and theme. The work is in omap.js (the
// boot ROM), service.js (backup, boot loader, restore) and dfu.js (the system).
import { romBoot, ROM_FILTER } from "./omap.js";
import { Dfu, DFU_FILTER } from "./dfu.js";
import { Service, ByteReader, readBackup, verifyBackup, NEVER_WRITTEN } from "./service.js";

const $ = id => document.getElementById(id);
const LANGS = ["en", "nl"];
const THEMES = ["default", "home-assistant", "light"];
// The USB serial gadget of the rescue kernel (Linux g_serial).
const SERIAL_FILTER = { usbVendorId: 0x0525, usbProductId: 0xa4a7 };
// Where the loaders go in RAM (tools/dfu-install/session1-stock.sh).
const ADDR_UBOOT = 0x80100000, ADDR_KERNEL = 0x80a00000;

// What each mode needs. "sha256" comes from images/manifest.json when the page has one.
const ROLES = {
  xload: { text: "files.xload" }, ubootram: { text: "files.ubootram" }, service: { text: "files.service" },
  uboot: { text: "files.uboot" }, installer: { text: "files.installer" }, rootfs: { text: "files.rootfs" }, kernel: { text: "files.kernel" },
  backupfile: { text: "files.backupfile", local: true },
};
const NEEDS = {
  install: ["xload", "ubootram", "service", "uboot", "installer", "rootfs", "kernel"],
  backup: ["xload", "ubootram", "service"],
  restore: ["xload", "ubootram", "service", "backupfile"],
};
const STEPS = {
  install: ["files", "rom", "svc", "backup", "uboot", "dfu", "done"],
  backup: ["files", "rom", "svc", "backup", "done"],
  restore: ["files", "rom", "svc", "restore", "done"],
};

let texts = {}, fallback = {}, mode = null, step = 0, manifest = null;
const files = {};        // role -> {name, data: Uint8Array | file: File, sum}
let service = null, port = null;

// ---------- texts and theme ----------
const store = { get(k) { try { return localStorage.getItem(k); } catch (e) { return null; } }, set(k, v) { try { localStorage.setItem(k, v); } catch (e) { /* private window */ } } };
const parse = text => Object.fromEntries(text.split("\n").filter(l => l && !l.startsWith("#") && l.includes("=")).map(l => [l.slice(0, l.indexOf("=")).trim(), l.slice(l.indexOf("=") + 1).trim()]));
const T = (k, vars) => { let s = texts[k] || fallback[k] || k; for (const [n, v] of Object.entries(vars || {})) s = s.replace("{" + n + "}", v); return s; };
async function getText(url) { const r = await fetch(url); if (!r.ok) throw new Error(url + ": " + r.status); return r.text(); }
async function setLang(code) {
  if (!LANGS.includes(code)) code = "en";
  try { texts = parse(await getText("lang/" + code + ".txt")); } catch (e) { texts = {}; code = "en"; }
  store.set("j49.lang", code); document.documentElement.lang = code; $("lang").value = code;
  document.title = T("title");
  document.querySelectorAll("[data-t]").forEach(e => e.textContent = T(e.dataset.t));
  $("lang").setAttribute("aria-label", T("language")); $("theme").setAttribute("aria-label", T("theme"));
  if (mode) paintFiles();
}
// The same theme files as the thermostat (tools/themes): key=RRGGBB lines.
async function setTheme(name) {
  if (!THEMES.includes(name)) name = "default";
  let t = {};
  try { t = { ...parse(await getText("themes/default.theme")), ...parse(await getText("themes/" + name + ".theme")) }; } catch (e) { /* keep the built-in colours */ }
  const root = document.documentElement.style;
  for (const k of ["bg", "surface", "surface2", "line", "dim", "text", "strong", "accent", "heat", "ok", "error", "info"]) {
    if (/^[0-9a-f]{6}$/i.test(t[k] || "")) root.setProperty("--" + k, "#" + t[k]);
  }
  const a = /^[0-9a-f]{6}$/i.test(t.accent || "") ? t.accent : "f0b429";
  const lum = (parseInt(a.slice(0, 2), 16) * 299 + parseInt(a.slice(2, 4), 16) * 587 + parseInt(a.slice(4, 6), 16) * 114) / 1000;
  root.setProperty("--on-accent", lum > 140 ? "#16130a" : "#ffffff");
  store.set("j49.theme", name); $("theme").value = name;
}

// ---------- small helpers ----------
function log(line) { const p = $("log"); p.textContent += line + "\n"; p.scrollTop = p.scrollHeight; }
function state(id, text, kind) { const e = $(id + "-state"); e.textContent = text; e.className = "state " + (kind || ""); }
function bar(id, done, total) { $(id + "-bar").style.width = (total ? Math.min(100, done * 100 / total) : 0) + "%"; }
const mb = n => (n / 1048576).toFixed(1);
const hex = buf => Array.from(new Uint8Array(buf), b => b.toString(16).padStart(2, "0")).join("");
function show() {
  const order = STEPS[mode];
  for (const s of ["files", "rom", "svc", "backup", "uboot", "dfu", "restore", "done"]) {
    const el = $("s-" + s), i = order.indexOf(s);
    el.hidden = i < 0;
    el.className = i < step ? "done" : i === step ? "active" : "later";
  }
  $("done-text").textContent = T("done." + mode);
}
function next() { step++; show(); const s = STEPS[mode][step]; if (s) $("s-" + s).scrollIntoView({ behavior: "smooth", block: "center" }); }
/** Run one step's work behind its button: errors go to the step's state line and the log. */
function guard(id, button, work) {
  button.onclick = async () => {
    button.disabled = true;
    try { await work(); }
    catch (e) {
      const cancelled = e && (e.name === "NotFoundError" || e.name === "AbortError");
      state(id, cancelled ? T("cancelled") : T("error") + ": " + (e && e.message || e), cancelled ? "" : "bad");
      if (!cancelled) log("ERROR " + (e && e.stack || e));
      button.disabled = false;
    }
  };
}

// ---------- step 1: files ----------
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
      const r = await fetch("images/" + m.file);
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
async function runService() {
  state("svc", "");
  port = await navigator.serial.requestPort({ filters: [SERIAL_FILTER] });
  await port.open({ baudRate: 115200, bufferSize: 1 << 20 });
  const rd = port.readable.getReader(), wr = port.writable.getWriter();
  const reader = new ByteReader(async () => { const { value, done } = await rd.read(); return done ? null : value; });
  service = new Service(reader, d => wr.write(d), log);
  state("svc", T("svc.waiting"));
  const parts = await service.info();
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
  const summary = await service.backup(sink, (d, t, n) => { bar("backup", d, t); state("backup", T("backup.reading", { name: n })); }, { date: new Date().toISOString() });
  await close();
  state("backup", T("backup.verify"));
  const file = await fileBack(), stream = file.stream().getReader();
  await verifyBackup(new ByteReader(async () => { const { value, done } = await stream.read(); return done ? null : value; }));
  const errors = summary.reduce((n, p) => n + p.X, 0);
  state("backup", T("backup.done", { mb: mb(file.size) }) + (errors ? " " + T("backup.errors", { n: errors }) : ""), errors ? "bad" : "ok");
  log("backup: " + file.size + " bytes, " + errors + " blocks with read errors");
  if (mode === "backup") { await service.reboot(); }
  next();
}

// ---------- step 5: the boot loader ----------
async function runUboot() {
  for (const name of ["spl0", "spl1"]) {
    state("uboot", T("uboot.writing", { name }));
    await service.writeImage(name, files.uboot.data, (d, t) => bar("uboot", (name === "spl1" ? t : 0) + d, 2 * t));
    log(name + " written and read back");
  }
  await service.reboot();
  try { await port.close(); } catch (e) { /* gone */ }
  state("uboot", T("uboot.done"), "ok");
  next();
}

// ---------- step 6: the system, over DFU ----------
async function dfuDevice(ask) {
  const known = (await navigator.usb.getDevices()).find(d => d.vendorId === DFU_FILTER.vendorId && d.productId === DFU_FILTER.productId);
  if (known) return known;
  return ask ? navigator.usb.requestDevice({ filters: [DFU_FILTER] }) : null;
}
/** Wait until U-Boot's DFU device is on the bus (again). */
function dfuBack() {
  return new Promise(resolve => {
    const seen = async () => { const d = await dfuDevice(false); if (d) { navigator.usb.removeEventListener("connect", seen); clearInterval(poll); resolve(d); } };
    navigator.usb.addEventListener("connect", seen);
    const poll = setInterval(seen, 3000);
    $("dfu-again").hidden = false;
    $("dfu-again").onclick = async () => { try { await navigator.usb.requestDevice({ filters: [DFU_FILTER] }); seen(); } catch (e) { /* nothing chosen */ } };
  });
}
async function runDfu() {
  let dfu = new Dfu(await dfuDevice(true), log);
  log("DFU targets: " + (await dfu.open()).join(", "));
  const total = files.installer.data.length + files.rootfs.data.length;
  state("dfu", T("dfu.stage2", { mb: mb(total) }));
  await dfu.download("installer", files.installer.data, d => bar("dfu", d, total));
  await dfu.download("rootfs", files.rootfs.data, d => bar("dfu", files.installer.data.length + d, total));
  await dfu.detach();
  state("dfu", T("dfu.installing"));
  // the device leaves the bus, installs, and comes back in the same DFU mode
  await new Promise(r => setTimeout(r, 15000));
  dfu = new Dfu(await dfuBack(), log);
  $("dfu-again").hidden = true;
  await dfu.open();
  state("dfu", T("dfu.stage3")); bar("dfu", 0, 1);
  await dfu.download("boot0", files.kernel.data, (d, t) => bar("dfu", d, t));
  // the installer kernel is still in RAM: overwrite its start, or U-Boot would run it again
  await dfu.download("installer", new Uint8Array(4));
  await dfu.detach();
  state("dfu", T("dfu.done"), "ok");
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
    const r = await service.writeFrames(p.name, p.frames, "exact", (d, t) => bar("restore", done + p.size * d / t, total));
    done += p.size;
    log(p.name + " written back" + (r.skipped ? " (" + r.skipped + " bad blocks left out)" : ""));
  }
  await service.reboot();
  try { await port.close(); } catch (e) { /* gone */ }
  state("restore", T("restore.done"), "ok");
  next();
}

// ---------- start ----------
for (const [id, list] of [["lang", LANGS], ["theme", THEMES]]) for (const v of list) { const o = document.createElement("option"); o.value = v; o.textContent = v; $(id).appendChild(o); }
$("lang").onchange = () => setLang($("lang").value);
$("theme").onchange = () => setTheme($("theme").value);
try { fallback = parse(await getText("lang/en.txt")); } catch (e) { fallback = {}; }
await setLang(store.get("j49.lang") || ((navigator.language || "en").slice(0, 2)));
await setTheme(store.get("j49.theme") || "default");
for (const code of LANGS) { try { const t = parse(await getText("lang/" + code + ".txt")); [...$("lang").options].find(o => o.value === code).textContent = t["lang.name"] || code; } catch (e) { /* keep the code */ } }
for (const o of $("theme").options) o.textContent = o.value.replace(/(^|-)([a-z])/g, (m, a, b) => (a ? " " : "") + b.toUpperCase());

const usable = !!(navigator.usb && navigator.serial);
$("nobrowser").hidden = usable;
$("insecure").hidden = window.isSecureContext;
$("windows").hidden = !/Windows/.test(navigator.userAgent);
await loadManifest();
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
guard("restore", $("restore-go"), runRestore);
