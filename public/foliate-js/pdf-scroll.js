// foliate-pdf-scroll — continuous vertical scroll renderer for PDFs.
//
// Mirrors the public surface of `foliate-fxl` enough for `foliate-view` to
// treat it as a drop-in PDF renderer:
//   - open(book), goTo(target), next(), prev(), getContents(), destroy()
//   - observes `zoom` and `spread` attributes
//   - dispatches `load`, `create-overlayer`, `relocate` events
//
// Layout is organized around **rows**. A row holds 1 page (single-page
// layout) or 2 pages side-by-side (two-page layout, `spread="auto"`).
// Rows are virtualized via IntersectionObserver: a lightweight placeholder
// div holds its natural size (from `book.getPageSize(i)`) until it enters
// a render buffer around the viewport, at which point iframes are mounted.
// When a row leaves a wider tear-down buffer, iframes are unmounted and
// the placeholder's size is preserved so scroll position stays stable.

const RENDER_BUFFER_VIEWPORTS = 2   // mount within 2 viewport heights
const UNLOAD_BUFFER_VIEWPORTS = 3   // unmount beyond 3 viewport heights

export class PDFScroll extends HTMLElement {
    static observedAttributes = ['zoom', 'spread', 'resize-dragging']
    #root = this.attachShadow({ mode: 'closed' })
    #container
    #rows = []
    #pageSizes = []
    #observer
    #zoom = 1             // number | 'fit-width' | 'fit-page'
    #spread = 'none'      // 'none' | 'auto'
    #scrollRaf = 0
    #resizeTimeout = 0
    #resizeObserver
    #reportedIndex = -1
    book

    constructor() {
        super()
        const sheet = new CSSStyleSheet()
        sheet.replaceSync(`
            :host {
                display: block;
                width: 100%;
                height: 100%;
                overflow: auto;
                background: var(--page-bg, transparent);
            }
            :host::-webkit-scrollbar { width: 12px; height: 12px; }
            :host::-webkit-scrollbar-track { background: transparent; }
            :host::-webkit-scrollbar-thumb { background: rgba(128,128,128,0.3); border-radius: 9999px; border: 3px solid transparent; background-clip: padding-box; }
            :host::-webkit-scrollbar-thumb:hover { background: rgba(128,128,128,0.5); border: 3px solid transparent; background-clip: padding-box; }
            .container {
                display: flex;
                flex-direction: column;
                align-items: center;
                gap: 12px;
                padding: 12px 0;
                min-height: 100%;
                /* Grow to the widest row so the host's horizontal scroll can
                 * reach content that overflows at high zoom (two-page spread
                 * at 170% is wider than a 1800px viewport). Without this,
                 * align-items: center splits the overflow evenly to both
                 * sides and the left half becomes unreachable. */
                width: max-content;
                min-width: 100%;
                box-sizing: border-box;
            }
            .row {
                position: relative;
                display: flex;
                flex-direction: row;
                gap: 0;
                box-shadow: 0 2px 8px rgba(0, 0, 0, 0.15);
                background: #fff;
                flex-shrink: 0;
            }
            .slot {
                position: relative;
                background: #fff;
                flex-shrink: 0;
                overflow: hidden;
            }
            .slot > iframe {
                border: 0;
                display: block;
                width: 100%;
                height: 100%;
                background: #fff;
            }
        `)
        this.#root.adoptedStyleSheets = [sheet]
        this.#container = document.createElement('div')
        this.#container.className = 'container'
        this.#root.append(this.#container)
        this.addEventListener('scroll', () => this.#onScroll())
    }

    relayout() {
        if (this.hasAttribute('resize-dragging')) return
        this.#layoutAll()
    }

    attributeChangedCallback(name, _, value) {
        if (name === 'zoom') {
            if (value === 'fit-width' || value === 'fit-page') {
                this.#zoom = value
            } else {
                const parsed = parseFloat(value)
                this.#zoom = isNaN(parsed) || parsed <= 0 ? 1 : parsed
            }
            if (this.book) this.#layoutAll()
        } else if (name === 'spread') {
            const next = value === 'none' ? 'none' : 'auto'
            if (next === this.#spread) return
            this.#spread = next
            if (this.book) this.#rebuildRows()
        } else if (name === 'resize-dragging') {
            if (value == null && this.book) this.#layoutAll()
        }
    }

    async open(book) {
        this.book = book
        // Inherit spread from book rendition the same way fixed-layout does.
        if (book.rendition?.spread === 'none') this.#spread = 'none'
        // Pre-fetch natural page sizes so placeholders can reserve space.
        const n = book.sections?.length ?? 0
        const getSize = typeof book.getPageSize === 'function'
            ? book.getPageSize.bind(book)
            : () => Promise.resolve({ width: 800, height: 1000 })
        this.#pageSizes = await Promise.all(
            Array.from({ length: n }, (_, i) => getSize(i).catch(() => ({ width: 800, height: 1000 })))
        )
        this.#rebuildRows()
        // Re-layout when the container is resized (e.g. side panel open/close).
        this.#resizeObserver?.disconnect()
        this.#resizeObserver = new ResizeObserver(() => {
            if (this.hasAttribute('resize-dragging')) return
            clearTimeout(this.#resizeTimeout)
            this.#resizeTimeout = setTimeout(() => this.#layoutAll(), 150)
        })
        this.#resizeObserver.observe(this)
    }

    #rebuildRows() {
        // Capture the currently-visible page so we can restore scroll after rebuild.
        const wasIndex = this.#reportedIndex >= 0 ? this.#reportedIndex : 0

        this.#observer?.disconnect()
        for (const row of this.#rows) {
            for (const slot of row.slots) this.#unmountSlot(slot)
        }
        this.#container.replaceChildren()
        this.#rows = []

        const n = this.#pageSizes.length
        const pairPages = this.#spread === 'auto'
        for (let i = 0; i < n;) {
            const leftSize = this.#pageSizes[i]
            const rightSize = pairPages && i + 1 < n ? this.#pageSizes[i + 1] : null
            const row = this.#createRow(i, leftSize, rightSize)
            this.#rows.push(row)
            i += rightSize ? 2 : 1
        }

        this.#layoutAll({ preservePosition: false })
        this.#setupObserver()

        // Restore scroll position to the row containing the previously-active page.
        const targetRow = this.#rows.find(r => r.slots.some(s => s.index === wasIndex))
        if (targetRow) this.#scrollRowToTop(targetRow)
        // Kick off an initial relocate so listeners know the current page.
        this.#reportLocation('page', targetRow ? wasIndex : null)
    }

    #createRow(firstIndex, leftSize, rightSize) {
        const element = document.createElement('div')
        element.className = 'row'
        element.dataset.rowIndex = String(this.#rows.length)

        const makeSlot = (index, size) => {
            const el = document.createElement('div')
            el.className = 'slot'
            element.append(el)
            return {
                index,
                element: el,
                iframe: null,
                loaded: false,
                loading: false,
                overlayer: null,
                srcRef: null,
                naturalWidth: size.width,
                naturalHeight: size.height,
            }
        }

        const slots = [makeSlot(firstIndex, leftSize)]
        if (rightSize) slots.push(makeSlot(firstIndex + 1, rightSize))

        this.#container.append(element)
        return { element, slots }
    }

    #setupObserver() {
        this.#observer?.disconnect()
        const rootMargin = `${RENDER_BUFFER_VIEWPORTS * 100}% 0px`
        this.#observer = new IntersectionObserver((entries) => {
            for (const entry of entries) {
                const rowIndex = parseInt(entry.target.dataset.rowIndex, 10)
                const row = this.#rows[rowIndex]
                if (!row) continue
                if (entry.isIntersecting) {
                    for (const slot of row.slots) this.#mountSlot(slot)
                } else {
                    // Out of render buffer — check distance; if beyond unload
                    // buffer, tear down.
                    const rect = entry.boundingClientRect
                    const rootHeight = this.clientHeight
                    const distance = rect.bottom < 0
                        ? -rect.bottom
                        : rect.top > rootHeight
                            ? rect.top - rootHeight
                            : 0
                    if (distance > UNLOAD_BUFFER_VIEWPORTS * rootHeight) {
                        for (const slot of row.slots) this.#unmountSlot(slot)
                    }
                }
            }
        }, {
            root: this,
            rootMargin,
            threshold: 0,
        })
        for (const row of this.#rows) this.#observer.observe(row.element)
    }

    async #mountSlot(slot) {
        if (slot.iframe || slot.loading) return
        slot.loading = true
        const section = this.book.sections[slot.index]
        let src
        try {
            src = await section.load()
        } catch (err) {
            console.error('pdf-scroll: failed to load section', slot.index, err)
            slot.loading = false
            return
        }
        if (!src) { slot.loading = false; return }
        // We might have been unmounted while awaiting load()
        if (!slot.element.isConnected) { slot.loading = false; return }

        slot.srcRef = src

        const iframe = document.createElement('iframe')
        iframe.setAttribute('sandbox', 'allow-same-origin allow-scripts')
        iframe.setAttribute('scrolling', 'no')
        const srcUrl = typeof src === 'string' ? src : src.src
        slot.element.append(iframe)
        slot.iframe = iframe

        iframe.addEventListener('load', () => {
            slot.loaded = true
            slot.loading = false
            const doc = iframe.contentDocument
            if (!doc) return
            // Fire `load` so `foliate-view` / `Reader.tsx` attach per-page
            // selection, context-menu, and keyboard listeners (identical
            // contract as `fixed-layout.js`).
            this.dispatchEvent(new CustomEvent('load', { detail: { doc, index: slot.index } }))

            // Render the canvas at the current scale via onZoom, then set up
            // the overlayer once rendering has settled.
            const scale = this.#currentScale()
            if (src && typeof src === 'object' && typeof src.onZoom === 'function') {
                const p = src.onZoom({ doc, scale })
                if (p && typeof p.then === 'function') {
                    p.then(() => this.#setupOverlayer(slot))
                } else {
                    // Best-effort: dispatch after a short delay to let the
                    // debounced render complete.
                    setTimeout(() => this.#setupOverlayer(slot), 250)
                }
            } else {
                this.#setupOverlayer(slot)
            }
        }, { once: true })

        iframe.src = srcUrl
    }

    #unmountSlot(slot) {
        if (slot.iframe) {
            slot.iframe.remove()
            slot.iframe = null
        }
        slot.loaded = false
        slot.loading = false
        if (slot.overlayer?.element) slot.overlayer.element.remove()
        slot.overlayer = null
    }

    #setupOverlayer(slot) {
        // If the slot was torn down while we were rendering, bail.
        if (!slot.iframe || !slot.iframe.contentDocument) return
        const doc = slot.iframe.contentDocument
        this.dispatchEvent(new CustomEvent('create-overlayer', {
            detail: {
                doc,
                index: slot.index,
                attach: (overlayer) => {
                    if (slot.overlayer?.element) slot.overlayer.element.remove()
                    slot.overlayer = overlayer
                    slot.element.style.position = 'relative'
                    slot.element.append(overlayer.element)
                },
            },
        }))
    }

    #currentScale() {
        if (typeof this.#zoom === 'number') return this.#zoom
        if (!this.#rows.length) return 1
        // Use the widest row for fit-width so pages don't overflow horizontally.
        const maxRowWidth = Math.max(
            ...this.#rows.map(r =>
                r.slots.reduce((w, s) => w + s.naturalWidth, 0)
            ),
            1,
        )
        const containerW = Math.max(this.clientWidth - 24, 1) // 12px padding each side
        if (this.#zoom === 'fit-width') return containerW / maxRowWidth
        // fit-page — pick whichever of width/height is tighter per the tallest row
        const maxRowHeight = Math.max(
            ...this.#rows.map(r => Math.max(...r.slots.map(s => s.naturalHeight))),
            1,
        )
        const containerH = Math.max(this.clientHeight - 24, 1)
        return Math.min(containerW / maxRowWidth, containerH / maxRowHeight)
    }

    #activationOffset() {
        return Math.min(this.clientHeight * 0.15, 120)
    }

    #rowTop(row) {
        const rect = row.element.getBoundingClientRect()
        const hostRect = this.getBoundingClientRect()
        return this.scrollTop + rect.top - hostRect.top
    }

    #rowForIndex(index) {
        return index >= 0
            ? this.#rows.find(r => r.slots.some(s => s.index === index))
            : null
    }

    #rowAtActivationLine() {
        const activationLine = this.scrollTop + this.#activationOffset()
        for (const row of this.#rows) {
            const top = this.#rowTop(row)
            const bottom = top + row.element.offsetHeight
            if (bottom >= activationLine) return row
        }
        return this.#rows[this.#rows.length - 1] ?? null
    }

    #captureScrollAnchor() {
        const row = this.#rowAtActivationLine()
            ?? this.#rowForIndex(this.#reportedIndex)
        if (!row) return null
        const height = row.element.offsetHeight
        const offset = this.scrollTop + this.#activationOffset() - this.#rowTop(row)
        const ratio = height > 0
            ? Math.max(0, Math.min(1, offset / height))
            : 0
        const slot = row.slots.find(s => s.index === this.#reportedIndex)
            ?? row.slots[0]
        return { index: slot.index, ratio }
    }

    #restoreScrollAnchor(anchor) {
        if (!anchor) return
        const row = this.#rowForIndex(anchor.index)
        if (!row) return
        const top = this.#rowTop(row)
        const offset = row.element.offsetHeight * anchor.ratio
        this.scrollTop = Math.max(0, top + offset - this.#activationOffset())
    }

    #layoutAll({ preservePosition = true } = {}) {
        if (!this.#rows.length) return
        const anchor = preservePosition ? this.#captureScrollAnchor() : null
        const scale = this.#currentScale()
        for (const row of this.#rows) {
            for (const slot of row.slots) {
                slot.element.style.width = `${slot.naturalWidth * scale}px`
                slot.element.style.height = `${slot.naturalHeight * scale}px`
            }
        }
        // Re-render loaded slots at the new scale via the onZoom hook.
        for (const row of this.#rows) {
            for (const slot of row.slots) {
                if (!slot.loaded || !slot.iframe) continue
                const src = slot.srcRef
                const doc = slot.iframe.contentDocument
                if (src && typeof src === 'object' && typeof src.onZoom === 'function' && doc) {
                    const p = src.onZoom({ doc, scale })
                    if (p && typeof p.then === 'function') {
                        p.then(() => this.#setupOverlayer(slot))
                    }
                }
            }
        }
        this.#restoreScrollAnchor(anchor)
        // Center horizontally after the new widths are applied. When content
        // is wider than the viewport, parking scrollLeft at the middle of the
        // overflow puts the row's midpoint at the viewport's midpoint — which
        // is what users expect when zooming into a centered page spread.
        requestAnimationFrame(() => {
            const overflow = this.scrollWidth - this.clientWidth
            this.scrollLeft = overflow > 0 ? overflow / 2 : 0
        })
    }

    #onScroll() {
        cancelAnimationFrame(this.#scrollRaf)
        this.#scrollRaf = requestAnimationFrame(() => this.#reportLocation('scroll'))
    }

    #reportLocation(reason, explicitIndex = null) {
        if (!this.#rows.length) return
        // Current page = the row crossing a top-biased reading line. Using
        // the viewport midpoint makes a lower visible page steal the active
        // TOC item while the previous page header is still the user's focus.
        let currentIndex = explicitIndex
        if (currentIndex == null) {
            const activationLine = this.scrollTop + this.#activationOffset()
            currentIndex = this.#rows[this.#rows.length - 1].slots[0].index
            for (const row of this.#rows) {
                const top = this.#rowTop(row)
                const bottom = top + row.element.offsetHeight
                if (bottom >= activationLine) {
                    const currentSlot = row.slots.find(slot => slot.index === this.#reportedIndex)
                    currentIndex = currentSlot?.index ?? row.slots[0].index
                    break
                }
            }
        }
        if (currentIndex === this.#reportedIndex && reason === 'scroll') return
        this.#reportedIndex = currentIndex
        // `foliate-view` expects `fraction` to be progress within the current
        // section. PDFs model each page as one section, so page-level reporting
        // should use the top of that section rather than a document fraction.
        this.dispatchEvent(new CustomEvent('relocate', {
            detail: { reason, range: null, index: currentIndex, fraction: 0, size: 1 },
        }))
    }

    get index() { return this.#reportedIndex }

    async goTo(target) {
        const resolved = await target
        const index = resolved?.index ?? 0
        const row = this.#rows.find(r => r.slots.some(s => s.index === index))
        const slot = row?.slots.find(s => s.index === index)
        if (!row || !slot) return
        this.#scrollSlotToTarget(slot, resolved)
        // Force an immediate relocate so listeners get the new index
        // even if the scroll event coalesces.
        this.#reportedIndex = -1
        this.#reportLocation('page', index)
    }

    // offsetTop is unreliable inside shadow DOM (the row's offsetParent
    // walks past the shadow boundary and returns viewport-relative
    // coordinates instead of scroll-container-relative ones), so compute
    // the scroll delta from bounding rects instead.
    #scrollElementToTop(element) {
        const rect = element.getBoundingClientRect()
        const hostRect = this.getBoundingClientRect()
        this.scrollTop += rect.top - hostRect.top
    }

    #scrollRowToTop(row) {
        this.#scrollElementToTop(row.element)
    }

    #scrollSlotToTarget(slot, target) {
        this.#scrollElementToTop(slot.element)
        const top = target?.anchor?.top
        if (typeof top !== 'number' || !Number.isFinite(top)) return
        const scale = slot.naturalHeight ? slot.element.offsetHeight / slot.naturalHeight : 1
        const offset = Math.max(0, Math.min(slot.naturalHeight, top)) * scale
        this.scrollTop += offset - this.clientHeight / 2
    }

    async next() {
        const total = this.#pageSizes.length
        const step = this.#spread === 'auto' ? 2 : 1
        return this.goTo({ index: Math.min((this.#reportedIndex < 0 ? 0 : this.#reportedIndex) + step, total - 1) })
    }

    async prev() {
        const step = this.#spread === 'auto' ? 2 : 1
        return this.goTo({ index: Math.max((this.#reportedIndex < 0 ? 0 : this.#reportedIndex) - step, 0) })
    }

    getContents() {
        const out = []
        for (const row of this.#rows) {
            for (const slot of row.slots) {
                if (slot.loaded && slot.iframe) {
                    out.push({
                        doc: slot.iframe.contentDocument,
                        index: slot.index,
                        overlayer: slot.overlayer,
                    })
                }
            }
        }
        return out
    }

    setStyles(_css) {
        // PDFs don't honor reader stylesheets — theming is done via the overlay
        // div in Reader.tsx. Accept the call for API compatibility.
    }

    destroy() {
        this.#observer?.disconnect()
        this.#observer = null
        this.#resizeObserver?.disconnect()
        this.#resizeObserver = null
        for (const row of this.#rows) {
            for (const slot of row.slots) this.#unmountSlot(slot)
        }
        this.#rows = []
        this.book = null
    }
}

customElements.define('foliate-pdf-scroll', PDFScroll)
