// The install band: pick a target, copy its command. Without JavaScript the page still shows
// the macOS arm64 command, selectable by hand.

const RELEASES = "https://github.com/filipecabaco/ampoule/releases/latest/download";

// The fixed part of the URL is a variable on its own line, so the line that changes with the
// target stays short enough to show whole. Returns the command split at the points it may wrap.
function installFor(target) {
  return {
    base: RELEASES.split("/").map((part, i, all) => (i < all.length - 1 ? `${part}/` : part)),
    asset: `"$base/ampoule-${target}"`,
  };
}

// Render with <wbr> after each "/" of the base URL: it wraps only there, never mid-word, and
// copying the text yields the plain command.
function render(code, target) {
  const { base, asset } = installFor(target);
  code.replaceChildren(document.createTextNode("base="));
  base.forEach((part) => {
    code.append(document.createTextNode(part), document.createElement("wbr"));
  });
  // The asset name is the part the target chips change: it never breaks, not even at a hyphen.
  const token = document.createElement("span");
  token.className = "token";
  token.textContent = asset;
  code.append(document.createTextNode("\ncurl -fLo ampoule "), token, document.createTextNode("\nchmod +x ampoule"));
}

// A best guess at the visitor's platform. Browsers do not report CPU architecture reliably, so
// macOS defaults to arm64 and Linux to x86_64; the visitor can switch.
function guessTarget() {
  const platform = (navigator.userAgentData && navigator.userAgentData.platform) || navigator.platform || "";
  if (/mac/i.test(platform)) return "aarch64-darwin";
  if (/linux/i.test(platform)) return /arm|aarch64/i.test(navigator.userAgent) ? "aarch64-linux" : "x86_64-linux";
  return null;
}

function setupTargets() {
  const code = document.querySelector("[data-install]");
  const radios = document.querySelectorAll('input[name="target"]');
  if (!code || radios.length === 0) return;

  const show = (target) => render(code, target);
  radios.forEach((radio) => radio.addEventListener("change", () => show(radio.value)));

  const guess = guessTarget();
  const match = guess && document.querySelector(`input[name="target"][value="${guess}"]`);
  if (match) {
    match.checked = true;
    show(guess);
  }
}

function setupCopy() {
  document.querySelectorAll("[data-copy]").forEach((button) => {
    const label = button.querySelector("[data-copy-label]");
    const icon = button.querySelector("use");
    button.addEventListener("click", async () => {
      const source = document.querySelector(button.dataset.copy);
      if (!source) return;
      try {
        await navigator.clipboard.writeText(source.textContent.trim());
        label.textContent = "Copied";
        icon.setAttribute("href", "#icon-check");
        // Copying a step's command marks that step done: its numeral fills gold.
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
      }
      setTimeout(() => {
        label.textContent = "Copy";
        icon.setAttribute("href", "#icon-copy");
      }, 1800);
    });
  });
}

document.documentElement.classList.add("js");
setupTargets();
setupCopy();
