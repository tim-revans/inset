// @ts-check
// The Stock tracker's page. It shows what the back end replies and sends
// what staff enter; every rule, such as what counts as low stock, is the
// back end's. `tsc` checks this file against the API contract that
// `inset build` writes, so a renamed field fails the check.

/** @typedef {import("../target/inset/stock-tracker").StockLevel} StockLevel */
/** @typedef {import("../target/inset/stock-tracker").Item} Item */

const levels = /** @type {HTMLTableSectionElement} */ (document.getElementById("levels"));
const lowOnly = /** @type {HTMLInputElement} */ (document.getElementById("low"));
const form = /** @type {HTMLFormElement} */ (document.getElementById("stock"));
const outcome = /** @type {HTMLElement} */ (document.getElementById("outcome"));

/** Lists stock levels: `GET /items?{low}`. */
async function list() {
  const query = lowOnly.checked ? "?low=true" : "";
  const response = await fetch(`/items${query}`);
  /** @type {StockLevel[]} */
  const stock = await response.json();
  levels.replaceChildren(...stock.map(row));
}

/** @param {StockLevel} level */
function row(level) {
  const tr = document.createElement("tr");
  tr.className = level.low ? "low" : "";
  for (const [text, number] of [
    [level.sku, false],
    [String(level.onHand), true],
    [String(level.reorderLevel), true],
    [level.low ? "low" : "", false],
  ]) {
    const td = document.createElement("td");
    td.textContent = /** @type {string} */ (text);
    td.className = number ? "number" : "";
    tr.append(td);
  }
  return tr;
}

/** Stocks an item: `PUT /items/{sku}` with the item as JSON. */
async function stock() {
  const data = new FormData(form);
  /** @type {Item} */
  const item = {
    onHand: Number(data.get("onHand")),
    reorderLevel: Number(data.get("reorderLevel")),
    targetLevel: Number(data.get("targetLevel")),
  };
  const sku = encodeURIComponent(String(data.get("sku")));
  const response = await fetch(`/items/${sku}`, {
    method: "PUT",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(item),
  });
  outcome.textContent = response.ok
    ? `Stocked with ${await response.text()} on hand.`
    : `Not stocked: the tracker replied ${response.status}.`;
  await list();
}

lowOnly.addEventListener("change", list);
form.addEventListener("submit", (event) => {
  event.preventDefault();
  stock();
});
list();
