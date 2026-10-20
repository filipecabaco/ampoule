// The two steps follow two choices: the machine that packs (what to download) and the machine
// that runs the file (the --target, the file step 2 copies, the part on the cover). Without
// JavaScript the page shows macOS arm64 packing for Linux x86_64, selectable by hand.

const RELEASES = "https://github.com/filipecabaco/ampoule/releases/latest/download";

function token(text) {
  const span = document.createElement("span");
  span.className = "token";
  span.textContent = text;
  return span;
}

function prompt() {
  const span = document.createElement("span");
  span.className = "p";
  span.textContent = "$";
  return span;
}

// The fixed part of the URL is a variable on its own line, with <wbr> after each "/", so it wraps
// only there and the lines that change with the choices stay short enough to show whole. The
// asset names are tokens: they never break, not even at a hyphen.
function renderInstall(code, host, ship) {
  code.replaceChildren(document.createTextNode("base="));
  RELEASES.split("/").forEach((part, i, all) => {
    code.append(document.createTextNode(i < all.length - 1 ? `${part}/` : part), document.createElement("wbr"));
  });
  code.append(document.createTextNode("\ncurl -fsSL "), token(`"$base/ampoule-${host}.tar.gz"`), document.createTextNode(" \\\n  | tar -xz -C ~"));
  // Packing for another platform starts from that platform's runtime, kept next to ampoule.
  if (ship !== host) {
    code.append(document.createTextNode(`\ncurl -fLo ~/ampoule/ampoule-${ship} \\\n  `), token(`"$base/ampoule-${ship}"`));
  }
  code.append(document.createTextNode('\nexport PATH="$HOME/ampoule:$PATH"'));
}

function render(host, ship) {
  const install = document.querySelector("[data-install]");
  if (install) renderInstall(install, host, ship);

  const release = document.querySelector("[data-release]");
  if (release) release.replaceChildren(prompt(), document.createTextNode(ship === host ? " ampoule release" : ` ampoule release --target ${ship}`));

  const file = `my_app-${ship}`;
  const copy = document.querySelector("[data-ship]");
  if (copy) {
    copy.replaceChildren(
      prompt(),
      document.createTextNode(` scp _build/prod/ampoule/${file} server:\n`),
      prompt(),
      document.createTextNode(` ssh server ./${file} start`),
    );
  }
  document.querySelectorAll("[data-ship-name]").forEach((el) => {
    el.textContent = file;
  });
}

// A best guess at the visitor's platform. Browsers do not report CPU architecture reliably, so
// macOS defaults to arm64 and Linux to x86_64; the visitor can switch.
function guessHost() {
  const platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
  if (/mac/i.test(platform)) return "aarch64-darwin";
  if (/linux/i.test(platform)) return /arm|aarch64/i.test(navigator.userAgent) ? "aarch64-linux" : "x86_64-linux";
  return null;
}

function setupTargets() {
  const radios = document.querySelectorAll('input[name="host"], input[name="ship"]');
  if (radios.length === 0) return;
  const chosen = (name) => document.querySelector(`input[name="${name}"]:checked`)?.value;
  const show = () => render(chosen("host"), chosen("ship"));
  radios.forEach((radio) => radio.addEventListener("change", show));

  const guess = guessHost();
  const match = guess && document.querySelector(`input[name="host"][value="${guess}"]`);
  if (match) match.checked = true;
  show();
}

// What a command plate copies: the commands, without the "$" prompts or any output shown.
function commandText(source) {
  const copy = source.cloneNode(true);
  copy.querySelectorAll(".p, .o").forEach((el) => el.remove());
  return copy.textContent
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .join("\n");
}

function setupCopy() {
  const status = document.querySelector("[data-copy-status]");
  document.querySelectorAll("[data-copy]").forEach((button) => {
    const label = button.querySelector("[data-copy-label]");
    const icon = button.querySelector("use");
    let reset;
    button.addEventListener("click", async () => {
      const source = document.querySelector(button.dataset.copy);
      if (!source) return;
      clearTimeout(reset);
      try {
        await navigator.clipboard.writeText(commandText(source));
        label.textContent = "Copied";
        icon.setAttribute("href", "#icon-check");
        if (status) status.textContent = "Copied to the clipboard.";
        // Copying a step's command marks that step done: its numeral fills blue.
        const step = button.closest("[data-step]");
        if (step) step.classList.add("is-done");
      } catch {
        // Clipboard refused (insecure context or permissions): select the text instead.
        const range = document.createRange();
        range.selectNodeContents(source);
        const selection = window.getSelection();
        selection.removeAllRanges();
        selection.addRange(range);
        label.textContent = "Selected";
        if (status) status.textContent = "Selected: copy it with your keyboard.";
      }
      reset = setTimeout(() => {
        label.textContent = "Copy";
        icon.setAttribute("href", "#icon-copy");
        if (status) status.textContent = "";
      }, 1800);
    });
  });
}

document.documentElement.classList.add("js");
setupTargets();
setupCopy();
