import init, { CdpExtractor, info_cdp } from "./pkg/cdptool_web.js";
import { Zip, ZipDeflate, ZipPassThrough } from "https://cdn.jsdelivr.net/npm/fflate@0.8.2/+esm";

const dropZone = document.getElementById("drop-zone");
const statusEl = document.getElementById("status");
const results = document.getElementById("results");
const resultsTitle = document.getElementById("results-title");
const downloadBtn = document.getElementById("download-btn");
const progressEl = document.getElementById("progress");
const progressFill = document.getElementById("progress-fill");
const progressText = document.getElementById("progress-text");
const infoOutput = document.getElementById("info-output");

let lastCdpBytes = null;
let lastName = null;
let extracting = false;

function showError(msg) {
  statusEl.textContent = msg;
  statusEl.hidden = false;
}
function hideError() { statusEl.hidden = true; }

function formatSize(bytes) {
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
  return (bytes / 1024 / 1024).toFixed(1) + " MB";
}

// Phase 1: drop CDP → parse and show structure immediately.
async function handleFile(file) {
  hideError();
  results.hidden = true;
  progressEl.hidden = true;
  lastCdpBytes = null;

  if (!file.name.endsWith(".cdp")) {
    showError("Expected a .cdp file.");
    return;
  }

  lastCdpBytes = new Uint8Array(await file.arrayBuffer());
  lastName = file.name.replace(/\.cdp$/, "");

  try {
    infoOutput.textContent = info_cdp(lastCdpBytes);
  } catch (e) {
    showError("Failed to parse: " + e);
    return;
  }

  results.hidden = false;
  resultsTitle.textContent = `${file.name} (${formatSize(lastCdpBytes.length)})`;
  downloadBtn.disabled = false;
  downloadBtn.textContent = "Download .zip";
}

// Phase 2: click download → extract files streaming into a zip download.
async function startDownload() {
  if (!lastCdpBytes || extracting) return;
  extracting = true;
  downloadBtn.disabled = true;
  downloadBtn.textContent = "Extracting...";
  progressEl.hidden = false;
  progressFill.style.width = "0%";
  progressText.textContent = "Starting...";

  let extractor;
  try {
    extractor = new CdpExtractor(lastCdpBytes);
  } catch (e) {
    showError("Parse error: " + e);
    resetDownloadState();
    return;
  }

  const total = extractor.total();

  // Collect zip chunks as they're produced by fflate
  const chunks = [];
  let zipSize = 0;

  const zip = new Zip((err, chunk, final) => {
    if (err) { showError("Zip error: " + err); return; }
    chunks.push(chunk);
    zipSize += chunk.length;
  });

  let done = 0;

  try {
    await new Promise((resolve, reject) => {
      function step() {
        try {
          const batchEnd = Math.min(done + 10, total);
          while (done < batchEnd) {
            const entry = extractor.next_file();
            if (entry === null) break;
            const [path, data] = [entry[0], entry[1]];

            // Use ZipDeflate for compressible files, ZipPassThrough for tiny ones
            const stream = data.length > 256
              ? new ZipDeflate(path, { level: 6 })
              : new ZipPassThrough(path);
            zip.add(stream);
            stream.push(data, true); // true = final chunk for this file

            done++;
          }
          const pct = total > 0 ? (done / total) * 100 : 0;
          progressFill.style.width = pct + "%";
          progressText.textContent = `Extracting ${done} / ${total} files`;
          if (done >= total) {
            zip.end();
            resolve();
          } else {
            requestAnimationFrame(step);
          }
        } catch (e) {
          reject(e);
        }
      }
      step();
    });
  } catch (e) {
    showError("Extraction error: " + e);
    extractor.free();
    resetDownloadState();
    return;
  }

  extractor.free();

  // Trigger download from collected chunks
  const blob = new Blob(chunks, { type: "application/zip" });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = lastName + ".zip";
  a.click();
  URL.revokeObjectURL(url);

  progressFill.style.width = "100%";
  progressText.textContent =
    `Done — ${formatSize(blob.size)} zip (${done} files)`;
  downloadBtn.disabled = false;
  downloadBtn.textContent = "Download .zip";
  extracting = false;
}

function resetDownloadState() {
  extracting = false;
  downloadBtn.disabled = false;
  downloadBtn.textContent = "Download .zip";
  progressEl.hidden = true;
}

downloadBtn.addEventListener("click", startDownload);

// Drag and drop
dropZone.addEventListener("dragover", (e) => {
  e.preventDefault();
  dropZone.classList.add("hover");
});
dropZone.addEventListener("dragleave", () => dropZone.classList.remove("hover"));
dropZone.addEventListener("drop", (e) => {
  e.preventDefault();
  dropZone.classList.remove("hover");
  if (e.dataTransfer.files.length > 0) handleFile(e.dataTransfer.files[0]);
});

dropZone.addEventListener("click", () => {
  const input = document.createElement("input");
  input.type = "file";
  input.accept = ".cdp";
  input.onchange = () => { if (input.files.length > 0) handleFile(input.files[0]); };
  input.click();
});

init().catch((e) => showError("Failed to load WASM module: " + e));
