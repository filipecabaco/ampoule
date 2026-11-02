// The copy buttons on the command plates. The commands themselves are static: the installer finds
// the visitor's platform, so the page has nothing to choose.

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
        if (status) status.textContent = "Selected: copy it now.";
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
setupCopy();
