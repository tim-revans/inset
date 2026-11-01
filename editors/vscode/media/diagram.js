// The diagram panel's page. It shows the SVG the extension sends, and tells
// the extension which element was clicked, by the ID in its `data-id`.
// Double-clicking a function shows its flowchart. With "Side by side" on,
// clicking a route instead picks it: the components a request to it holds
// are shaded, and every other route is marked by whether a request to it
// can run meanwhile, which it can when the two hold no component in common.

// @ts-check
(function () {
  // @ts-ignore: provided by VS Code inside a webview.
  const vscode = acquireVsCodeApi();
  const view = /** @type {HTMLSelectElement} */ (document.getElementById("view"));
  const expandAll = /** @type {HTMLInputElement} */ (document.getElementById("expand-all"));
  const coverage = /** @type {HTMLInputElement} */ (document.getElementById("coverage"));
  const sideBySide = /** @type {HTMLInputElement} */ (document.getElementById("side-by-side"));
  const side = /** @type {HTMLElement} */ (document.getElementById("side"));
  /** Each route, with the components a request to it holds. @type {{ handler: string, route: string, holds: string[] }[]} */
  let routes = [];
  /** The route picked, by its handler's ID. @type {string | null} */
  let picked = null;
  const message = /** @type {HTMLElement} */ (document.getElementById("message"));
  const problems = /** @type {HTMLElement} */ (document.getElementById("problems"));
  const picture = /** @type {HTMLElement} */ (document.getElementById("picture"));
  /** @type {string[]} */
  let functions = [];

  function sendView() {
    vscode.postMessage({
      type: "view",
      flowchart: view.value,
      expandAll: expandAll.checked,
      coverage: coverage.checked,
    });
  }
  view.addEventListener("change", sendView);
  expandAll.addEventListener("change", sendView);
  coverage.addEventListener("change", sendView);

  /** The innermost element with an ID that leads to its source. */
  function target(/** @type {EventTarget | null} */ node) {
    return node instanceof Element ? node.closest("[data-source]") : null;
  }

  picture.addEventListener("click", (event) => {
    const element = target(event.target);
    if (!element) {
      return;
    }
    const id = element.getAttribute("data-id");
    if (sideBySide.checked && routes.some((r) => r.handler === id)) {
      picked = id;
      markSideBySide();
      return;
    }
    vscode.postMessage({ type: "open", id });
  });

  sideBySide.addEventListener("change", markSideBySide);

  /** The SVG elements with a `data-id`, by ID. */
  function byId(/** @type {string} */ id) {
    return [...picture.querySelectorAll("[data-id]")].filter((e) => e.getAttribute("data-id") === id);
  }

  /**
   * Shades what the picked route holds and marks the other routes, or
   * clears the marks when "Side by side" is off.
   */
  function markSideBySide() {
    for (const mark of picture.querySelectorAll(".side-mark")) {
      mark.remove();
    }
    for (const element of picture.querySelectorAll(".side-held")) {
      element.classList.remove("side-held");
    }
    if (!sideBySide.checked) {
      side.hidden = true;
      return;
    }
    side.hidden = false;
    const chosen = routes.find((r) => r.handler === picked);
    if (!chosen) {
      side.textContent = routes.length
        ? "Click a route to see which requests can run while one to it does."
        : "This program has no routes.";
      return;
    }
    for (const component of chosen.holds) {
      for (const box of byId(component)) {
        if (box.tagName === "rect") {
          box.classList.add("side-held");
        }
      }
    }
    const alongside = [];
    const waits = [];
    for (const route of routes) {
      const state =
        route === chosen ? "chosen" : route.holds.some((c) => chosen.holds.includes(c)) ? "waits" : "alongside";
      if (state === "alongside") alongside.push(route.route);
      if (state === "waits") waits.push(route.route);
      for (const row of byId(route.handler)) {
        if (!(row instanceof SVGGraphicsElement) || row.tagName !== "g") {
          continue;
        }
        const box = row.getBBox();
        const ns = "http://www.w3.org/2000/svg";
        const band = document.createElementNS(ns, "rect");
        band.setAttribute("x", String(box.x - 4));
        band.setAttribute("y", String(box.y - 2));
        band.setAttribute("width", String(box.width + 8));
        band.setAttribute("height", String(box.height + 4));
        band.setAttribute("rx", "3");
        band.setAttribute("class", `side-mark side-${state}`);
        row.parentNode?.insertBefore(band, row);
        const label = document.createElementNS(ns, "text");
        label.setAttribute("x", String(box.x + box.width + 10));
        label.setAttribute("y", String(box.y + box.height - 3));
        label.setAttribute("class", `side-mark side-label side-${state}`);
        label.textContent = state === "chosen" ? "picked" : state;
        row.parentNode?.insertBefore(label, row.nextSibling);
      }
    }
    const names = (/** @type {string[]} */ list) => list.join(", ");
    side.textContent =
      `${chosen.route} holds ${chosen.holds.map((c) => c.split("/").pop()).join(" and ")}. ` +
      (alongside.length ? `Runs alongside ${names(alongside)}. ` : "Runs alongside no other route. ") +
      (waits.length ? `Takes turns with ${names(waits)}.` : "");
  }

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
    coverage.checked = data.coverage;
    routes = data.routes ?? [];

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
    markSideBySide();
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
      case "side-by-side":
        // Sent by the extension's tests.
        sideBySide.checked = data.on;
        markSideBySide();
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
