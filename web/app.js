import init, { extract_cdp, create_cdp, info_cdp } from "./pkg/cdptool_web.js";

const dropZone = document.getElementById("drop-zone");
const status = document.getElementById("status");
const results = document.getElementById("results");
const resultsTitle = document.getElementById("results-title");
const fileList = document.getElementById("file-list");
const infoBtn = document.getElementById("info-btn");
const infoOutput = document.getElementById("info-output");

let lastCdpBytes = null;

function showStatus(msg, type) {
  status.textContent = msg;
  status.className = type;
  status.hidden = false;
}

function hideStatus() {
  status.hidden = true;
}

function formatSize(bytes) {
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
  return (bytes / 1024 / 1024).toFixed(1) + " MB";
}

function downloadBlob(data, filename) {
  const blob = new Blob([data]);
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

function displayExtracted(extracted, sourceName) {
  results.hidden = false;
  resultsTitle.textContent = `Extracted ${extracted.length} files from ${sourceName}`;
  fileList.innerHTML = "";
  infoBtn.hidden = false;
  infoOutput.hidden = true;

  for (const pair of extracted) {
    const [name, data] = [pair[0], pair[1]];
    const row = document.createElement("div");
    const a = document.createElement("a");
    a.textContent = name;
    a.href = "#";
    a.onclick = (e) => { e.preventDefault(); downloadBlob(data, name); };
    const size = document.createElement("span");
    size.className = "size";
    size.textContent = formatSize(data.length);
    row.appendChild(a);
    row.appendChild(size);
    fileList.appendChild(row);
  }
}

function displayCreated(cdpBytes) {
  results.hidden = false;
  resultsTitle.textContent = `Created archive (${formatSize(cdpBytes.length)})`;
  fileList.innerHTML = "";
  infoBtn.hidden = false;
  infoOutput.hidden = true;

  const a = document.createElement("a");
  a.textContent = "Download archive.cdp";
  a.href = "#";
  a.onclick = (e) => { e.preventDefault(); downloadBlob(cdpBytes, "archive.cdp"); };
  fileList.appendChild(a);
}

async function handleDrop(files) {
  hideStatus();
  results.hidden = true;
  lastCdpBytes = null;

  if (files.length === 1 && files[0].name.endsWith(".cdp")) {
    showStatus("Unpacking...", "working");
    try {
      const buf = new Uint8Array(await files[0].arrayBuffer());
      lastCdpBytes = buf;
      const extracted = extract_cdp(buf);
      hideStatus();
      displayExtracted(extracted, files[0].name);
    } catch (e) {
      showStatus("Error: " + e, "error");
    }
  } else {
    showStatus("Packing " + files.length + " file(s)...", "working");
    try {
      const pairs = await Promise.all(
        [...files].map(async (f) => [f.name, new Uint8Array(await f.arrayBuffer())])
      );
      const cdpBytes = create_cdp(pairs);
      lastCdpBytes = cdpBytes;
      hideStatus();
      displayCreated(cdpBytes);
    } catch (e) {
      showStatus("Error: " + e, "error");
    }
  }
}

// Drag and drop
dropZone.addEventListener("dragover", (e) => {
  e.preventDefault();
  dropZone.classList.add("hover");
});

dropZone.addEventListener("dragleave", () => {
  dropZone.classList.remove("hover");
});

dropZone.addEventListener("drop", (e) => {
  e.preventDefault();
  dropZone.classList.remove("hover");
  if (e.dataTransfer.files.length > 0) {
    handleDrop([...e.dataTransfer.files]);
  }
});

// Click to open file picker
dropZone.addEventListener("click", () => {
  const input = document.createElement("input");
  input.type = "file";
  input.multiple = true;
  input.onchange = () => {
    if (input.files.length > 0) handleDrop([...input.files]);
  };
  input.click();
});

// Info button
infoBtn.addEventListener("click", () => {
  if (!lastCdpBytes) return;
  if (!infoOutput.hidden) {
    infoOutput.hidden = true;
    infoBtn.textContent = "Show structure";
    return;
  }
  try {
    infoOutput.textContent = info_cdp(lastCdpBytes);
    infoOutput.hidden = false;
    infoBtn.textContent = "Hide structure";
  } catch (e) {
    showStatus("Error: " + e, "error");
  }
});

// Init WASM
init().then(() => {
  dropZone.querySelector("p").insertAdjacentText("beforebegin", "");
}).catch((e) => {
  showStatus("Failed to load WASM module: " + e, "error");
});
