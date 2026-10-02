/* Shared, compact context menus. App-specific admission stays in showCtxMenu. */
window.ContextMenu = (() => {
  "use strict";
  const focusable = "button,input,select,textarea,a[href],[tabindex]";

  function create({ fallbackFocus = () => [], onChange = () => {} } = {}) {
    let current = null, invocation = null, keyboardTarget = null;
    const visible = node => !!node?.isConnected && !node.hidden
      && !node.closest?.('[hidden],[inert],[aria-hidden="true"]')
      && !!node.getClientRects?.().length && getComputedStyle(node).visibility !== "hidden";
    const validFocus = node => visible(node) && !node.disabled && node.getAttribute("aria-disabled") !== "true"
      && node.matches?.(focusable) && !node.closest(".ctx-menu");
    const returnTarget = entry => [entry.origin, ...fallbackFocus(entry.source)].find(validFocus);
    const consume = event => { event.preventDefault(); event.stopImmediatePropagation(); };
    const controls = () => current ? [...current.menu.querySelectorAll('[role="menuitem"]')].filter(visible) : [];
    const scrollPosition = node => {
      const element = node === document ? document.scrollingElement || document.documentElement : node;
      return [element.scrollLeft || 0, element.scrollTop || 0];
    };
    function captureScroll(source) {
      const positions = new Map([[document, scrollPosition(document)]]);
      for (let node = source; node; node = node.parentElement) positions.set(node, scrollPosition(node));
      return positions;
    }

    function close({ restoreFocus = false } = {}) {
      if (!current) return;
      const entry = current;
      current = null;
      entry.menu.remove();
      onChange(null);
      if (restoreFocus) returnTarget(entry)?.focus({ preventScroll: true });
    }
    function position() {
      if (!current) return;
      const { menu, x, y } = current, bounds = menu.getBoundingClientRect();
      const width = document.documentElement.clientWidth, height = document.documentElement.clientHeight;
      menu.style.left = `${Math.max(8, Math.min(x, width - bounds.width - 8))}px`;
      menu.style.top = `${Math.max(8, Math.min(y, height - bounds.height - 8))}px`;
    }
    function open(x, y, items, { trigger = null, label = "Ações do contexto" } = {}) {
      // Native dispatch can run microtasks between capture and target callbacks.
      // eventPhase stays nonzero for the dispatch, then resets before later work.
      const input = invocation?.event.eventPhase ? invocation : null;
      const source = trigger || input?.target || document.activeElement;
      // Keep the original caller across a menu replaced by a paging action.
      const scope = source?.closest?.(".modal-overlay,[role='dialog'],.drawer");
      const origin = [source?.closest?.(focusable), current?.origin, document.activeElement]
        .find(node => validFocus(node) && (!scope || scope.contains(node)));
      const keyboard = keyboardTarget || input?.keyboard;
      close();
      const menu = document.createElement("div");
      menu.className = "ctx-menu";
      menu.setAttribute("role", "menu");
      menu.setAttribute("aria-label", label);
      menu.tabIndex = -1;
      Object.assign(menu.style, { boxSizing: "border-box", minWidth: "min(230px, calc(100vw - 16px))",
        maxWidth: "calc(100vw - 16px)", maxHeight: "calc(100vh - 16px)", overflowY: "auto" });
      for (const item of items) {
        if (item.sep) {
          const separator = document.createElement("div");
          separator.className = "ctx-sep"; separator.setAttribute("role", "separator");
          menu.appendChild(separator); continue;
        }
        const button = document.createElement("button"), icon = document.createElement("i"), text = document.createElement("span");
        button.type = "button";
        button.className = "ctx-item" + (item.danger ? " danger" : "");
        button.setAttribute("role", "menuitem");
        button.tabIndex = -1;
        // Disabled actions remain discoverable in a menu's arrow-key sequence.
        if (item.disabled) button.setAttribute("aria-disabled", "true");
        if (item.title) { button.title = item.title; button.setAttribute("aria-description", item.title); }
        icon.className = `fas ${item.icon || ""}`;
        icon.setAttribute("aria-hidden", "true");
        if (item.color) icon.style.color = item.color;
        text.textContent = item.label;
        button.append(icon, text);
        button.onclick = () => {
          if (button.getAttribute("aria-disabled") === "true" || current?.menu !== menu) return;
          const entry = current;
          close({ restoreFocus: true });
          item.onClick?.();
          // A synchronous delete/re-render can remove the restored caller. Do
          // not override focus deliberately moved into a new menu or editor.
          if (!current && (document.activeElement === document.body
            || document.activeElement === entry.origin && !validFocus(entry.origin))) {
            returnTarget(entry)?.focus({ preventScroll: true });
          }
        };
        menu.appendChild(button);
      }
      const anchor = source?.getBoundingClientRect?.();
      current = { menu, source, origin, scrollPositions: captureScroll(source),
        x: keyboard && anchor ? anchor.left : Number.isFinite(x) ? x : anchor?.left || 8,
        y: keyboard && anchor ? anchor.bottom : Number.isFinite(y) ? y : anchor?.bottom || 8 };
      document.body.appendChild(menu);
      onChange(menu);
      position();
      (controls()[0] || menu).focus({ preventScroll: true });
      return menu;
    }
    // Capture the actual caller before legacy handlers run. Never retain a stale
    // input event for a later async menu or rely on the browser's window.event.
    function remember(event) {
      if (current?.menu.contains(event.target)) return;
      if (current) close();
      const entry = { event, target: event.target, keyboard: !!keyboardTarget || event.detail === 0 && event.clientX === 0 && event.clientY === 0 };
      invocation = entry;
      // Release the event next task, but only open() during its actual dispatch
      // may use it. A timer alone would leak the caller into async menu opens.
      setTimeout(() => { if (invocation === entry) invocation = null; }, 0);
    }
    document.addEventListener("click", remember, true);
    document.addEventListener("contextmenu", remember, true);
    document.addEventListener("focusin", event => {
      if (current && !current.menu.contains(event.target)) close();
    }, true);
    window.addEventListener("keydown", event => {
      if (event.isComposing || event.keyCode === 229) return;
      if (current) {
        if (event.key === "Escape") { consume(event); close({ restoreFocus: true }); return; }
        if (!current.menu.contains(event.target)) return;
        if (event.key === "Tab") {
          // Let normal Tab order (and an enclosing modal's trap) run from the caller.
          close({ restoreFocus: true }); return;
        }
        const items = controls(), index = items.indexOf(document.activeElement);
        if (["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) {
          consume(event);
          const next = event.key === "Home" ? 0 : event.key === "End" ? items.length - 1
            : event.key === "ArrowDown" ? (index + 1) % items.length : index < 0 ? items.length - 1 : (index - 1 + items.length) % items.length;
          items[next]?.focus({ preventScroll: true });
          items[next]?.scrollIntoView({ block: "nearest" });
        } else if (event.key === "Enter" || event.key === " ") {
          consume(event);
          if (!event.repeat) items[index]?.click();
        } else if (["ArrowLeft", "ArrowRight", "ContextMenu"].includes(event.key) || event.key === "F10" && event.shiftKey) {
          consume(event); // No submenus; never move the event drawer underneath.
        }
        return;
      }
      if (event.defaultPrevented || event.repeat || event.ctrlKey || event.metaKey || event.altKey
        || !(event.key === "ContextMenu" || event.key === "F10" && event.shiftKey)) return;
      const target = document.activeElement;
      if (!validFocus(target)) return;
      // Reuse the existing caller and its actions without adding every table cell
      // to the tab order. Delegated, non-focusable text still needs its own model.
      let caller = target;
      while (caller && typeof caller.oncontextmenu !== "function") caller = caller.parentElement;
      if (!caller) return;
      consume(event);
      keyboardTarget = target;
      try {
        const bounds = target.getBoundingClientRect();
        target.dispatchEvent(new MouseEvent("contextmenu", { bubbles: true, cancelable: true,
          clientX: bounds.left, clientY: bounds.bottom, button: 2 }));
      } finally { keyboardTarget = null; }
    }, true);
    document.addEventListener("scroll", event => {
      if (!current || current.menu.contains(event.target)) return;
      const before = current.scrollPositions.get(event.target);
      if (!before) return;
      const after = scrollPosition(event.target);
      // A right-click may follow auto-scrolling the caller into view before its
      // queued scroll event arrives. Dismiss only for movement after menu open.
      if (after.some((value, index) => value !== before[index])) close();
    }, true);
    window.addEventListener("resize", position);
    return { open, close, get element() { return current?.menu || null; } };
  }
  return { create };
})();
