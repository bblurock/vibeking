import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";
import { ChevronDown, Search, Star } from "lucide-react";
import { cn } from "@/lib/utils";

type Option = { value: string; label: string };

type Props = {
  value: string;
  onChange: (value: string) => void;
  options: Option[];
  placeholder?: string;
  searchPlaceholder?: string;
  emptyLabel?: string;
  className?: string;
  /// Min width of the popover. Defaults to the trigger width.
  panelMinWidth?: number;
  /// Optional favorite-toggle hooks. When both are provided, the dropdown
  /// renders a star button per row, favorited rows sort to the top with a
  /// thin separator below them, and toggling fires onToggleFavorite
  /// without closing the panel — so the user can star several languages
  /// in one open. `isFavoritable(value)` lets callers exclude sentinel
  /// options like "auto" that are always implicit.
  isFavorite?: (value: string) => boolean;
  onToggleFavorite?: (value: string) => void;
  isFavoritable?: (value: string) => boolean;
  favoriteAddLabel?: string;
  favoriteRemoveLabel?: string;
};

/// Searchable single-select with a portal-mounted popover.
///
/// Built ground-up instead of pulling in cmdk / headlessui so the only
/// dependency stays Tailwind + the existing design tokens. The panel is
/// rendered into `document.body` via createPortal — settings rows live
/// inside cards with `overflow-hidden`, so an `absolute`-positioned
/// child would get clipped on long lists like Whisper-99. Fixed
/// positioning anchored to the trigger's `getBoundingClientRect()`
/// avoids that without forcing every ancestor to opt into visible
/// overflow.
export function Combobox({
  value,
  onChange,
  options,
  placeholder = "Select…",
  searchPlaceholder = "Search…",
  emptyLabel = "No match",
  className,
  panelMinWidth,
  isFavorite,
  onToggleFavorite,
  isFavoritable,
  favoriteAddLabel = "Add to menu bar quick-switch",
  favoriteRemoveLabel = "Remove from menu bar quick-switch",
}: Props) {
  const favoritesEnabled = Boolean(isFavorite && onToggleFavorite);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const [panelRect, setPanelRect] = useState<{
    left: number;
    top: number;
    width: number;
  } | null>(null);
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const panelRef = useRef<HTMLDivElement | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);
  const listboxId = useId();

  const selected = options.find((o) => o.value === value);

  // Snapshot the favorite-sort key at panel-open time. The row positions
  // are derived from this snapshot, NOT from the live `isFavorite`
  // function — so toggling a star while the panel is open updates the
  // star icon's filled/empty state without reordering the rows under
  // the user's pointer. Without this, unstarring item N causes every
  // item below to shift up by one row and the next click lands on the
  // wrong row.
  const [favSortSnapshot, setFavSortSnapshot] = useState<Set<string>>(
    () => new Set(),
  );

  // Apply the query first, then split favorites to the top so the
  // separator goes between two filtered groups (not "all favorites,
  // then a filtered tail"). Each entry knows whether it's the last
  // favorite — that's where the separator renders. The displayed star
  // state reads from `isFavorite` live (so toggles feel instant); only
  // the sort position uses the snapshot.
  type Row = Option & {
    favorite: boolean;
    sortFavorite: boolean;
    lastFavorite: boolean;
  };
  const filtered: Row[] = useMemo(() => {
    const q = query.trim().toLowerCase();
    const matches = q
      ? options.filter(
          (o) =>
            o.label.toLowerCase().includes(q) ||
            o.value.toLowerCase().includes(q),
        )
      : options;

    if (!favoritesEnabled) {
      return matches.map((o) => ({
        ...o,
        favorite: false,
        sortFavorite: false,
        lastFavorite: false,
      }));
    }

    const favs: Option[] = [];
    const rest: Option[] = [];
    for (const o of matches) {
      if (favSortSnapshot.has(o.value)) favs.push(o);
      else rest.push(o);
    }
    return [
      ...favs.map((o, i) => ({
        ...o,
        favorite: isFavorite!(o.value),
        sortFavorite: true,
        lastFavorite: i === favs.length - 1 && rest.length > 0,
      })),
      ...rest.map((o) => ({
        ...o,
        favorite: isFavorite!(o.value),
        sortFavorite: false,
        lastFavorite: false,
      })),
    ];
  }, [options, query, favoritesEnabled, isFavorite, favSortSnapshot]);

  // Recompute the panel anchor from the trigger's bounding rect. Called
  // on open and whenever the viewport changes while the panel is open
  // (scroll / resize), since `position: fixed` doesn't follow the
  // trigger automatically.
  const reposition = useCallback(() => {
    const el = triggerRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    setPanelRect({
      left: rect.left,
      top: rect.bottom + 4,
      width: rect.width,
    });
  }, []);

  useLayoutEffect(() => {
    if (!open) return;
    reposition();
  }, [open, reposition]);

  useEffect(() => {
    if (!open) return;
    const handler = () => reposition();
    // capture=true picks up scrolls inside the settings card too.
    window.addEventListener("scroll", handler, true);
    window.addEventListener("resize", handler);
    return () => {
      window.removeEventListener("scroll", handler, true);
      window.removeEventListener("resize", handler);
    };
  }, [open, reposition]);

  // Keep the highlighted index in range as the filter shrinks the list.
  useEffect(() => {
    if (activeIndex >= filtered.length) {
      setActiveIndex(Math.max(0, filtered.length - 1));
    }
  }, [filtered.length, activeIndex]);

  // Close on outside click / Esc. Listen at the document level rather
  // than wiring blur on the trigger because focus moves between
  // trigger / search input / option, and blur would fire mid-flight.
  useEffect(() => {
    if (!open) return;
    function onDocMouseDown(e: MouseEvent) {
      const t = e.target as Node;
      if (
        triggerRef.current?.contains(t) ||
        panelRef.current?.contains(t)
      ) {
        return;
      }
      setOpen(false);
    }
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === "Escape") {
        setOpen(false);
        triggerRef.current?.focus();
      }
    }
    document.addEventListener("mousedown", onDocMouseDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onDocMouseDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  // Focus the search box and seed the highlight + favorite-sort
  // snapshot — but only on the closed→open transition, not on every
  // re-render while the panel is open. Settings updates from a star
  // toggle re-render the parent, which passes in a new `isFavorite`
  // function reference; without the transition guard the snapshot
  // regenerates and the rows resort under the user's pointer.
  //
  // activeIndex is seeded to 0 (not the current value's position) so
  // the existing scrollIntoView in the active-index effect anchors
  // the panel at the top — favorites first, which is what the user
  // came here to see.
  const lastOpenRef = useRef(false);
  useEffect(() => {
    const wasOpen = lastOpenRef.current;
    lastOpenRef.current = open;
    if (!open || wasOpen) return;
    setQuery("");
    setActiveIndex(0);
    if (favoritesEnabled && isFavorite) {
      const snap = new Set<string>();
      for (const o of options) {
        if (isFavorite(o.value)) snap.add(o.value);
      }
      setFavSortSnapshot(snap);
    }
    requestAnimationFrame(() => {
      inputRef.current?.focus();
      if (listRef.current) listRef.current.scrollTop = 0;
    });
  }, [open, options, favoritesEnabled, isFavorite]);

  // Keep the highlighted row scrolled into view as the user arrows
  // through a long list (Whisper-99 wraps off-screen quickly).
  useEffect(() => {
    if (!open || !listRef.current) return;
    const li = listRef.current.children[activeIndex] as
      | HTMLElement
      | undefined;
    li?.scrollIntoView({ block: "nearest" });
  }, [activeIndex, open]);

  function commit(option: Option) {
    onChange(option.value);
    setOpen(false);
    triggerRef.current?.focus();
  }

  function onListKeyDown(e: React.KeyboardEvent) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      setActiveIndex((i) => Math.min(filtered.length - 1, i + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      setActiveIndex((i) => Math.max(0, i - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      const opt = filtered[activeIndex];
      if (opt) commit(opt);
    }
  }

  const panel =
    open && panelRect ? (
      <div
        ref={panelRef}
        className={cn(
          "fixed z-[1000]",
          "rounded-lg border border-[var(--vk-border-strong)] bg-[var(--vk-surface)]",
          "shadow-[0_12px_36px_var(--vk-shadow-12),0_2px_6px_var(--vk-shadow-05)]",
          "overflow-hidden",
        )}
        style={{
          left: panelRect.left,
          top: panelRect.top,
          minWidth: Math.max(panelRect.width, panelMinWidth ?? 240),
        }}
        onKeyDown={onListKeyDown}
      >
        <div className="flex items-center gap-2 px-2.5 h-9 border-b border-[var(--vk-border)]">
          <Search
            className="size-3.5 text-[var(--vk-text-8)] shrink-0"
            aria-hidden="true"
          />
          <input
            ref={inputRef}
            type="text"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setActiveIndex(0);
            }}
            placeholder={searchPlaceholder}
            spellCheck={false}
            autoComplete="off"
            className={cn(
              "w-full h-full bg-transparent text-[12.5px] text-[var(--vk-text-2)]",
              "placeholder:text-[var(--vk-text-10)] focus:outline-none",
            )}
          />
        </div>
        <ul
          ref={listRef}
          id={listboxId}
          role="listbox"
          className="max-h-[280px] overflow-y-auto py-1"
        >
          {filtered.length === 0 ? (
            <li className="px-3 py-2 text-[12px] text-[var(--vk-text-9)]">
              {emptyLabel}
            </li>
          ) : (
            filtered.map((o, i) => {
              const isSelected = o.value === value;
              const isActive = i === activeIndex;
              const canFavorite =
                favoritesEnabled &&
                (isFavoritable ? isFavoritable(o.value) : true);
              return (
                <li
                  key={o.value}
                  role="option"
                  aria-selected={isSelected}
                  onMouseEnter={() => setActiveIndex(i)}
                  onMouseDown={(e) => {
                    // mousedown (not click) so the input doesn't blur
                    // before commit on macOS.
                    e.preventDefault();
                    commit(o);
                  }}
                  className={cn(
                    "px-3 py-1.5 text-[12.5px] cursor-pointer flex items-center justify-between gap-2",
                    isActive
                      ? "bg-[var(--vk-canvas-3)] text-[var(--vk-text)]"
                      : "text-[var(--vk-text-2)]",
                    o.lastFavorite &&
                      "border-b border-[var(--vk-border)] mb-1 pb-1.5",
                  )}
                >
                  <span className="truncate flex-1">{o.label}</span>
                  {isSelected && !canFavorite ? (
                    <span
                      aria-hidden
                      className="text-[10px] font-semibold text-[var(--vk-text-8)] tabular-nums"
                    >
                      {o.value}
                    </span>
                  ) : null}
                  {canFavorite ? (
                    <button
                      type="button"
                      // tabIndex=-1 keeps the search input as the focus
                      // anchor while the panel is open. Without this,
                      // clicking the star moves focus into the listbox
                      // and the browser scrolls the row into view —
                      // visible as the panel jumping mid-click.
                      tabIndex={-1}
                      aria-label={
                        o.favorite ? favoriteRemoveLabel : favoriteAddLabel
                      }
                      title={
                        o.favorite ? favoriteRemoveLabel : favoriteAddLabel
                      }
                      onMouseDown={(e) => {
                        // Stop the row's commit handler from firing — the
                        // star toggle is a separate intent from selecting
                        // the language as the active STT input. preventDefault
                        // on mousedown also stops the button from taking
                        // focus on macOS WebKit (belt-and-braces with
                        // tabIndex above).
                        e.preventDefault();
                        e.stopPropagation();
                        onToggleFavorite!(o.value);
                      }}
                      onClick={(e) => {
                        // Defensive: some browsers fire click even after
                        // preventDefault on mousedown. Swallow it so we
                        // don't double-toggle or steal focus.
                        e.preventDefault();
                        e.stopPropagation();
                      }}
                      className={cn(
                        "shrink-0 grid place-items-center size-5 rounded",
                        "hover:bg-[var(--vk-surface-3)] transition-colors",
                        o.favorite
                          ? "text-[var(--vk-accent-2)]"
                          : "text-[var(--vk-text-10)] hover:text-[var(--vk-text-8)]",
                      )}
                    >
                      <Star
                        className="size-3.5"
                        fill={o.favorite ? "currentColor" : "none"}
                        strokeWidth={o.favorite ? 0 : 1.5}
                        aria-hidden
                      />
                    </button>
                  ) : null}
                </li>
              );
            })
          )}
        </ul>
      </div>
    ) : null;

  return (
    <div className={cn("relative inline-flex", className)}>
      <button
        ref={triggerRef}
        type="button"
        onClick={() => setOpen((o) => !o)}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-controls={listboxId}
        className={cn(
          "appearance-none cursor-pointer h-7 pl-2.5 pr-7 text-[12px] relative",
          "rounded-md border border-[var(--vk-border-strong)] bg-[var(--vk-surface)]",
          "text-[var(--vk-text-2)] font-medium text-left min-w-[180px]",
          "hover:bg-[var(--vk-canvas-3)] transition-colors",
          "focus:outline-none focus:ring-2 focus:ring-[--color-ring] focus:ring-offset-1",
        )}
      >
        <span className="truncate block">
          {selected ? selected.label : placeholder}
        </span>
        <ChevronDown
          className="pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 size-3.5 text-[var(--vk-text-8)]"
          aria-hidden="true"
        />
      </button>
      {panel ? createPortal(panel, document.body) : null}
    </div>
  );
}
