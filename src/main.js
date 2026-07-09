const VISIBLE_ITEM_TTL_MS = 3 * 60 * 1000;
const DEFAULT_WIDGET_WIDTH = 360;
const DEFAULT_WIDGET_HEIGHT = 430;
const MIN_WIDGET_WIDTH = 300;
const MIN_WIDGET_HEIGHT = 260;
const MAX_WIDGET_WIDTH = 640;
const MAX_WIDGET_HEIGHT = 760;
const MAX_RENDERED_ITEMS = 40;
const COLLAPSED_DRAG_THRESHOLD = 6;

const fallbackItems = [
  {
    id: "fake-verge-nothing-phone-3",
    title: "Nothing Phone (3) confirmed to launch in July",
    url: "https://www.theverge.com/",
    source: "The Verge",
    timestamp: Math.floor(Date.now() / 1000) - 120,
    rawScore: 92,
    importance: 82,
    relevance: 42,
    tag: "Breaking",
    section: "Tech"
  },
  {
    id: "fake-wccftech-9950x-review",
    title: "AMD Ryzen 9 9950X review: insane performance gains",
    url: "https://wccftech.com/",
    source: "Wccftech",
    timestamp: Math.floor(Date.now() / 1000) - 18 * 60,
    rawScore: 67,
    importance: 48,
    relevance: 72,
    tag: "Watching",
    section: "Tech"
  },
  {
    id: "fake-techcrunch-gpt-4o-voice",
    title: "OpenAI unveils GPT-4o with real-time voice",
    url: "https://techcrunch.com/",
    source: "TechCrunch",
    timestamp: Math.floor(Date.now() / 1000) - 60 * 60,
    rawScore: 58,
    importance: 52,
    relevance: 88,
    tag: "General",
    section: "Tech"
  },
  {
    id: "fake-ars-recall-ai",
    title: "Microsoft brings AI features to Windows 11 Recall",
    url: "https://arstechnica.com/",
    source: "Ars Technica",
    timestamp: Math.floor(Date.now() / 1000) - 2 * 60 * 60,
    rawScore: 76,
    importance: 74,
    relevance: 51,
    tag: "Breaking",
    section: "Tech"
  },
  {
    id: "fake-xda-android-15-preview",
    title: "Android 15 Developer Preview 2 arrives with new APIs",
    url: "https://www.xda-developers.com/",
    source: "XDA Developers",
    timestamp: Math.floor(Date.now() / 1000) - 3 * 60 * 60,
    rawScore: 36,
    importance: 28,
    relevance: 20,
    tag: "General",
    section: "General"
  }
];

const appState = {
  collapsed: false,
  items: [],
  unread: 0,
  width: DEFAULT_WIDGET_WIDTH,
  height: DEFAULT_WIDGET_HEIGHT,
  activeSection: localStorage.getItem("kaapiwire.section") || "Tech",
  lastUpdateAt: Date.now()
};

const tauriApi = window.__TAURI__;
const invoke = tauriApi?.core?.invoke;
const listen = tauriApi?.event?.listen;

const elements = {
  html: document.documentElement,
  widget: document.querySelector("#widget"),
  widgetHeader: document.querySelector("#widgetHeader"),
  itemList: document.querySelector("#itemList"),
  tabs: [...document.querySelectorAll(".section-tab")],
  collapsedToggle: document.querySelector("#collapsedToggle"),
  collapseButton: document.querySelector("#collapseButton"),
  resizeHandle: document.querySelector("#resizeHandle"),
  unreadBadge: document.querySelector("#unreadBadge"),
  lastUpdate: document.querySelector("#lastUpdate")
};

elements.collapsedToggle.addEventListener("click", handleCollapsedClick);
elements.collapsedToggle.addEventListener("pointerdown", startCollapsedPointer);
elements.collapsedToggle.addEventListener("pointermove", moveCollapsedPointer);
elements.collapsedToggle.addEventListener("pointerup", endCollapsedPointer);
elements.collapsedToggle.addEventListener("pointercancel", endCollapsedPointer);
elements.collapseButton.addEventListener("click", () => setCollapsed(true));
elements.widgetHeader.addEventListener("pointerdown", startHeaderDrag);
elements.resizeHandle.addEventListener("pointerdown", startNativeResize);
window.addEventListener("resize", () => {
  if (!appState.collapsed) {
    appState.width = clamp(window.innerWidth, MIN_WIDGET_WIDTH, MAX_WIDGET_WIDTH);
    appState.height = clamp(window.innerHeight, MIN_WIDGET_HEIGHT, MAX_WIDGET_HEIGHT);
    queueSizeSave();
  }
  updateViewportLayout();
});

for (const tab of elements.tabs) {
  tab.addEventListener("click", () => {
    appState.activeSection = tab.dataset.section;
    localStorage.setItem("kaapiwire.section", appState.activeSection);
    render();
  });
}

elements.itemList.addEventListener("click", async (event) => {
  const headline = event.target.closest("[data-url]");
  if (!headline) {
    return;
  }

  const url = headline.dataset.url;
  if (invoke) {
    await invoke("open_url", { url });
    return;
  }

  window.open(url, "_blank", "noopener,noreferrer");
});

init();

async function init() {
  updateViewportLayout();

  if (invoke) {
    const config = await invoke("get_config");
    appState.collapsed = Boolean(config.collapsed);
    appState.width = clamp(config.width || DEFAULT_WIDGET_WIDTH, MIN_WIDGET_WIDTH, MAX_WIDGET_WIDTH);
    appState.height = clamp(config.height || DEFAULT_WIDGET_HEIGHT, MIN_WIDGET_HEIGHT, MAX_WIDGET_HEIGHT);
    receiveItems(await invoke("get_initial_items"), {
      resetUnread: !appState.collapsed,
      seedUnread: appState.collapsed
    });

    if (listen) {
      await listen("news:new-items", (event) => {
        receiveItems(event.payload.items, { generatedAt: event.payload.generatedAt });
      });
      await listen("news:snapshot", (event) => {
        replaceItems(event.payload.items, { generatedAt: event.payload.generatedAt });
      });
    }
  } else {
    receiveItems(fallbackItems, { resetUnread: true });
  }

  render();
  await syncNativeWindowSize();
  setInterval(updateRelativeTimes, 1000);
  setInterval(() => {
    const changed = pruneExpiredItems();
    if (changed) {
      render();
    }
  }, 5000);
}

function replaceItems(items, options = {}) {
  const receivedAt = options.generatedAt
    ? options.generatedAt * 1000
    : Date.now();

  appState.items = items
    .map((item) => ({ ...item, receivedAt }))
    .filter((item) => !isExpired(item))
    .sort((a, b) => b.receivedAt - a.receivedAt || b.timestamp - a.timestamp)
    .slice(0, MAX_RENDERED_ITEMS);
  appState.lastUpdateAt = receivedAt;

  if (!appState.collapsed) {
    appState.unread = 0;
  }

  render();
}

async function setCollapsed(collapsed) {
  appState.collapsed = collapsed;

  if (!collapsed) {
    appState.unread = 0;
  }

  render();
  await syncNativeWindowSize();
}

function receiveItems(items, options = {}) {
  const existingIds = new Set(appState.items.map((item) => item.id));
  const nextItems = [...appState.items];
  let newVisibleItems = 0;
  const receivedAt = options.generatedAt
    ? options.generatedAt * 1000
    : Date.now();

  for (const item of items) {
    if (existingIds.has(item.id)) {
      continue;
    }
    nextItems.push({ ...item, receivedAt });
    newVisibleItems += 1;
  }

  appState.items = nextItems
    .filter((item) => !isExpired(item))
    .sort((a, b) => b.receivedAt - a.receivedAt || b.timestamp - a.timestamp)
    .slice(0, MAX_RENDERED_ITEMS);
  appState.lastUpdateAt = receivedAt;

  if (options.seedUnread) {
    appState.unread = newVisibleItems > 0
      ? Math.min(9, Math.max(3, newVisibleItems))
      : 0;
  } else if (options.resetUnread || !appState.collapsed) {
    appState.unread = 0;
  } else {
    appState.unread = Math.min(9, appState.unread + newVisibleItems);
  }

  render();
}

function render() {
  pruneExpiredItems();
  elements.html.dataset.collapsed = String(appState.collapsed);
  elements.unreadBadge.textContent = String(appState.unread || 3);
  elements.unreadBadge.hidden = !appState.collapsed || appState.unread === 0;

  for (const tab of elements.tabs) {
    tab.classList.toggle("is-active", tab.dataset.section === appState.activeSection);
  }

  const visibleItems = appState.items.filter((item) => (item.section || "Tech") === appState.activeSection);
  const nodes = visibleItems.length
    ? visibleItems.map((item) => createItemNode(item))
    : [createEmptyNode()];
  elements.itemList.replaceChildren(...nodes);

  updateRelativeTimes();
}

function createItemNode(item) {
  const article = document.createElement("article");
  article.className = "news-item";
  article.setAttribute("role", "listitem");

  const meta = document.createElement("div");
  meta.className = "item-meta";
  meta.textContent = `${item.source}  -  ${relativeTime(item.timestamp)}`;

  const tag = document.createElement("span");
  tag.className = `tag tag-${item.tag.toLowerCase()}`;
  tag.textContent = item.tag.toUpperCase();

  const headline = document.createElement("button");
  headline.className = "headline";
  headline.type = "button";
  headline.dataset.url = item.url;
  headline.textContent = item.title;

  article.append(meta, tag, headline);
  return article;
}

function createEmptyNode() {
  const empty = document.createElement("div");
  empty.className = "empty-state";
  empty.textContent = `Listening for ${appState.activeSection.toLowerCase()} news`;
  return empty;
}

async function syncNativeWindowSize() {
  if (!invoke) {
    return;
  }

  updateViewportLayout();

  await invoke("set_collapsed", {
    collapsed: appState.collapsed,
    width: appState.width,
    height: appState.height,
    viewport: getViewportMetrics()
  });
}

function updateRelativeTimes() {
  for (const item of appState.items) {
    const node = elements.itemList.querySelector(`[data-url="${cssEscape(item.url)}"]`);
    const meta = node?.parentElement?.querySelector(".item-meta");
    if (meta) {
      meta.textContent = `${item.source}  -  ${relativeTime(item.timestamp)}`;
    }
  }

  elements.lastUpdate.textContent = `last update ${relativeTime(
    Math.floor(appState.lastUpdateAt / 1000)
  )}`;
}

function updateViewportLayout() {
  const maxWidgetHeight = Math.max(MIN_WIDGET_HEIGHT, Math.min(MAX_WIDGET_HEIGHT, screen.availHeight - 32));
  const maxWidgetWidth = Math.max(MIN_WIDGET_WIDTH, Math.min(MAX_WIDGET_WIDTH, screen.availWidth - 32));
  appState.width = clamp(appState.width, MIN_WIDGET_WIDTH, maxWidgetWidth);
  appState.height = clamp(appState.height, MIN_WIDGET_HEIGHT, maxWidgetHeight);
  elements.html.style.setProperty("--widget-width", `${appState.width}px`);
  elements.html.style.setProperty("--widget-height", `${appState.height}px`);
}

function getViewportMetrics() {
  return {
    availLeft: Number(screen.availLeft || 0),
    availTop: Number(screen.availTop || 0),
    availWidth: Number(screen.availWidth || screen.width),
    availHeight: Number(screen.availHeight || screen.height)
  };
}

function pruneExpiredItems() {
  const before = appState.items.length;
  appState.items = appState.items.filter((item) => !isExpired(item));
  return before !== appState.items.length;
}

function isExpired(item) {
  const receivedAt = item.receivedAt || appState.lastUpdateAt;
  return Date.now() - receivedAt > VISIBLE_ITEM_TTL_MS;
}

async function startHeaderDrag(event) {
  if (
    !invoke ||
    appState.collapsed ||
    event.button !== 0 ||
    event.target.closest("button")
  ) {
    return;
  }

  event.preventDefault();
  await invoke("start_drag");
}

let collapsedPointer = null;
let suppressCollapsedClick = false;

function startCollapsedPointer(event) {
  if (!invoke || !appState.collapsed || event.button !== 0) {
    return;
  }

  collapsedPointer = {
    pointerId: event.pointerId,
    startX: event.clientX,
    startY: event.clientY,
    lastScreenX: event.screenX,
    lastScreenY: event.screenY,
    dragging: false
  };
  elements.collapsedToggle.setPointerCapture?.(event.pointerId);
}

async function moveCollapsedPointer(event) {
  if (
    !invoke ||
    !collapsedPointer ||
    collapsedPointer.pointerId !== event.pointerId
  ) {
    return;
  }

  const deltaX = event.clientX - collapsedPointer.startX;
  const deltaY = event.clientY - collapsedPointer.startY;
  if (!collapsedPointer.dragging && Math.hypot(deltaX, deltaY) < COLLAPSED_DRAG_THRESHOLD) {
    return;
  }

  collapsedPointer.dragging = true;
  suppressCollapsedClick = true;
  const moveX = Math.round(event.screenX - collapsedPointer.lastScreenX);
  const moveY = Math.round(event.screenY - collapsedPointer.lastScreenY);
  collapsedPointer.lastScreenX = event.screenX;
  collapsedPointer.lastScreenY = event.screenY;
  event.preventDefault();

  if (moveX !== 0 || moveY !== 0) {
    await invoke("move_window_by", { deltaX: moveX, deltaY: moveY });
  }
}

function endCollapsedPointer(event) {
  if (!collapsedPointer || collapsedPointer.pointerId !== event.pointerId) {
    return;
  }

  if (collapsedPointer.dragging) {
    suppressCollapsedClick = true;
  }
  collapsedPointer = null;
}

function handleCollapsedClick(event) {
  if (suppressCollapsedClick) {
    event.preventDefault();
    suppressCollapsedClick = false;
    return;
  }

  setCollapsed(false);
}

async function startNativeResize(event) {
  if (!invoke || appState.collapsed) {
    return;
  }

  event.preventDefault();
  await invoke("start_resize");
}

let saveSizeTimer = 0;
function queueSizeSave() {
  clearTimeout(saveSizeTimer);
  saveSizeTimer = setTimeout(() => {
    if (!invoke || appState.collapsed) {
      return;
    }

    invoke("save_widget_size", {
      width: appState.width,
      height: appState.height
    }).catch(() => {});
  }, 250);
}

function clamp(value, min, max) {
  return Math.min(max, Math.max(min, Number(value) || min));
}

function relativeTime(timestampSeconds) {
  const elapsed = Math.max(0, Math.floor(Date.now() / 1000) - timestampSeconds);

  if (elapsed < 5) {
    return "just now";
  }

  if (elapsed < 60) {
    return `${elapsed}s ago`;
  }

  const minutes = Math.floor(elapsed / 60);
  if (minutes < 60) {
    return `${minutes}m ago`;
  }

  const hours = Math.floor(minutes / 60);
  return `${hours}h ago`;
}

function cssEscape(value) {
  if (window.CSS?.escape) {
    return CSS.escape(value);
  }

  return value.replace(/"/g, '\\"');
}
