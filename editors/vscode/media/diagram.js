// The diagram panel's page. It shows the SVG the extension sends, and tells
// the extension which element was clicked, by the ID in its `data-id`.
// Double-clicking a function shows its flowchart.

// @ts-check
(function () {
  // @ts-ignore: provided by VS Code inside a webview.
  const vscode = acquireVsCodeApi();
  const view = /** @type {HTMLSelectElement} */ (document.getElementById("view"));
  const expandAll = /** @type {HTMLInputElement} */ (document.getElementById("expand-all"));
  const message = /** @type {HTMLElement} */ (document.getElementById("message"));
  const problems = /** @type {HTMLElement} */ (document.getElementById("problems"));
  const picture = /** @type {HTMLElement} */ (document.getElementById("picture"));
  /** @type {string[]} */
  let functions = [];

  function sendView() {
    vscode.postMessage({ type: "view", flowchart: view.value, expandAll: expandAll.checked });
  }
  view.addEventListener("change", sendView);
  expandAll.addEventListener("change", sendView);

  /** The innermost element with an ID that leads to its source. */
  function target(/** @type {EventTarget | null} */ node) {
    return node instanceof Element ? node.closest("[data-source]") : null;
  }

  picture.addEventListener("click", (event) => {
    const element = target(event.target);
    if (element) {
      vscode.postMessage({ type: "open", id: element.getAttribute("data-id") });
    }
  });

  picture.addEventListener("dblclick", (event) => {
    const id = target(event.target)?.getAttribute("data-id");
    if (id && functions.includes(id)) {
      view.value = id;
      sendView();
    }
  });

  problems.addEventListener("click", (event) => {
    const item = event.target instanceof Element ? event.target.closest("li") : null;
    if (item) {
      vscode.postMessage({ type: "problem", index: Number(item.dataset.index) });
    }
  });

  /** A function's ID without the app's name, as `Greeter/fn:hello`. */
  function label(/** @type {string} */ id, /** @type {string | null} */ app) {
    const prefix = app ? `${app}/` : "";
    return `Flowchart of ${id.startsWith(prefix) ? id.slice(prefix.length) : id}`;
  }

  function show(/** @type {any} */ data) {
    functions = data.functions;
    view.replaceChildren(new Option("Components", ""));
    for (const id of functions) {
      view.add(new Option(label(id, data.app), id));
    }
    view.value = data.flowchart ?? "";
    expandAll.checked = data.expandAll;
    expandAll.disabled = Boolean(data.flowchart);

    problems.replaceChildren();
    data.problems.forEach((/** @type {string} */ text, /** @type {number} */ index) => {
      const item = document.createElement("li");
      item.textContent = text;
      item.dataset.index = String(index);
      problems.append(item);
    });
    problems.hidden = data.problems.length === 0;
    message.hidden = true;

    // The SVG comes from the Inset compiler, which escapes every label.
    picture.innerHTML = data.svg ?? "";
    const clickable = new Set(data.clickable);
    /** @type {string[]} */
    const ids = [];
    for (const element of picture.querySelectorAll("[data-id]")) {
      const id = element.getAttribute("data-id") ?? "";
      ids.push(id);
      if (clickable.has(id)) {
        element.setAttribute("data-source", "");
      }
    }
    vscode.postMessage({ type: "shown", ids });
  }

  window.addEventListener("message", (event) => {
    const data = event.data;
    switch (data.type) {
      case "picture":
        show(data);
        break;
      case "message":
        message.textContent = data.text;
        message.hidden = false;
        problems.hidden = true;
        picture.replaceChildren();
        break;
      case "click": {
        // Sent by the extension's tests, to click as a reader would.
        const element = [...picture.querySelectorAll("[data-id]")].find(
          (e) => e.getAttribute("data-id") === data.id,
        );
        element?.dispatchEvent(new MouseEvent("click", { bubbles: true }));
        break;
      }
    }
  });

  vscode.postMessage({ type: "ready" });
})();
