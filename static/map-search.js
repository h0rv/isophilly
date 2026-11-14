// @ts-check

export const SEARCH_CONFIG = {
  endpoint:
    "https://geocode.arcgis.com/arcgis/rest/services/World/GeocodeServer/findAddressCandidates",
  bounds: [-75.3, 39.86, -74.95, 40.14],
  city: "Philadelphia",
  limit: 5,
  timeout: 10000,
};
/** @typedef {{ label: string, longitude: number, latitude: number }} SearchResult */

/** @param {string} query @param {AbortSignal} signal @returns {Promise<SearchResult[]>} */
export async function searchAddresses(query, signal) {
  query = query.trim();
  if (query.length < 3 || query.length > 160)
    throw new Error("Enter an address or place in Philadelphia.");
  const url = new URL(SEARCH_CONFIG.endpoint);
  url.search = new URLSearchParams({
    SingleLine: /philadelphia/i.test(query) ? query : `${query}, ${SEARCH_CONFIG.city}, PA`,
    searchExtent: SEARCH_CONFIG.bounds.join(","),
    sourceCountry: "USA",
    maxLocations: String(SEARCH_CONFIG.limit),
    outFields: "City",
    outSR: "4326",
    forStorage: "false",
    f: "json",
  }).toString();
  const response = await fetch(url, {
    cache: "no-store",
    signal: AbortSignal.any([signal, AbortSignal.timeout(SEARCH_CONFIG.timeout)]),
  });
  if (!response.ok) throw new Error("Address search is unavailable. Please try again.");
  const data = await response.json();
  if (data.error || !Array.isArray(data.candidates))
    throw new Error("Address search is unavailable. Please try again.");
  const [west, south, east, north] = SEARCH_CONFIG.bounds;
  const results = [];
  for (const candidate of data.candidates) {
    if (
      candidate === null ||
      typeof candidate !== "object" ||
      typeof candidate.address !== "string" ||
      candidate.address.length >= 500 ||
      !Number.isFinite(candidate.score) ||
      candidate.score < 80 ||
      !Number.isFinite(candidate.location?.x) ||
      !Number.isFinite(candidate.location?.y) ||
      candidate.location.x < west ||
      candidate.location.x > east ||
      candidate.location.y < south ||
      candidate.location.y > north ||
      (candidate.attributes?.City && !/^philadelphia$/i.test(candidate.attributes.City))
    )
      continue;
    results.push({
      label: candidate.address,
      longitude: candidate.location.x,
      latitude: candidate.location.y,
    });
    if (results.length === SEARCH_CONFIG.limit) break;
  }
  return results;
}

/**
 * Only explicit submissions contact the geocoder. No autocomplete, history,
 * tracking, saved coordinates or address data in the URL.
 * @param {{form: HTMLFormElement, input: HTMLInputElement, results: HTMLElement, onSelect: (result: SearchResult) => boolean}} options
 */
export function createAddressSearch({ form, input, results, onSelect }) {
  /** @type {AbortController | undefined} */
  let controller;
  let lastSubmit = -Infinity;
  const button = form.querySelector("button[type=submit]");
  function cancel() {
    controller?.abort();
    controller = undefined;
    if (button instanceof HTMLButtonElement) button.disabled = false;
    results.replaceChildren();
  }
  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    if (performance.now() - lastSubmit < 1000) return;
    lastSubmit = performance.now();
    cancel();
    const request = new AbortController();
    controller = request;
    results.textContent = "Searching…";
    if (button instanceof HTMLButtonElement) button.disabled = true;
    try {
      const matches = await searchAddresses(input.value, request.signal);
      if (controller !== request) return;
      results.replaceChildren();
      if (!matches.length)
        results.textContent = "No Philadelphia match. Try a street number and street name.";
      for (const match of matches) {
        const candidate = document.createElement("button");
        candidate.type = "button";
        candidate.textContent = match.label;
        candidate.addEventListener("click", () => {
          if (!onSelect(match))
            results.textContent = "That result is outside this map. Try a Philadelphia address.";
        });
        results.append(candidate);
      }
    } catch (error) {
      if (controller === request && !request.signal.aborted) {
        results.textContent =
          error instanceof TypeError
            ? "Address search is unavailable. Check your connection and try again."
            : error instanceof Error && error.name !== "TimeoutError"
              ? error.message
              : "Search timed out. Please try again.";
      }
    } finally {
      if (controller === request && button instanceof HTMLButtonElement) button.disabled = false;
    }
  });
  return { cancel };
}
