const pdfjsPath = path => new URL(`vendor/pdfjs/${path}`, import.meta.url).toString()

import './vendor/pdfjs/pdf.mjs'
const pdfjsLib = globalThis.pdfjsLib
// Workers are constructed per-document in makePDF() and passed via
// `workerPort`. We deliberately don't set `workerSrc`: under custom
// schemes (tauri://, asset://) pdf.js treats the page origin as opaque
// ("null") and wraps the worker in a blob: that re-imports the worker
// URL. macOS 14.x WKWebView stalls silently on that pattern. Going
// through `workerPort` routes us into pdf.js's `#initializeFromPort`,
// which skips the blob wrapper entirely.

const fetchText = async url => await (await fetch(url)).text()

// https://raw.githubusercontent.com/mozilla/pdf.js/refs/tags/v5.5.207/web/text_layer_builder.css
const textLayerBuilderCSS = await fetchText(pdfjsPath('text_layer_builder.css'))

// https://raw.githubusercontent.com/mozilla/pdf.js/refs/tags/v5.5.207/web/annotation_layer_builder.css
const annotationLayerBuilderCSS = await fetchText(pdfjsPath('annotation_layer_builder.css'))

const render = async (page, doc, zoom) => {
    const scale = zoom * devicePixelRatio
    doc.documentElement.style.transform = `scale(${1 / devicePixelRatio})`
    doc.documentElement.style.transformOrigin = 'top left'
    doc.documentElement.style.setProperty('--scale-factor', scale)
    const viewport = page.getViewport({ scale })

    // the canvas must be in the `PDFDocument`'s `ownerDocument`
    // (`globalThis.document` by default); that's where the fonts are loaded
    const canvas = document.createElement('canvas')
    canvas.height = viewport.height
    canvas.width = viewport.width
    const canvasContext = canvas.getContext('2d')
    await page.render({ canvasContext, viewport }).promise
    doc.querySelector('#canvas').replaceChildren(doc.adoptNode(canvas))

    // pdfjs TextLayer.render() only appends spans — without clearing first,
    // re-renders (zoom, resize) stack duplicate transparent spans on top of
    // each other and the overlapping hit-targets make selection feel sloppy.
    const container = doc.querySelector('.textLayer')
    container.replaceChildren()
    // Match the params upstream pdfjs's text_layer_builder.js uses:
    //   - includeMarkedContent groups spans by their PDF logical structure
    //     (paragraphs, etc.) which keeps DOM order closer to reading order
    //     and gives the selection algorithm a saner tree to walk.
    //   - disableNormalization keeps the original glyph order so spans land
    //     in the same sequence as the canvas draws them.
    const textLayer = new pdfjsLib.TextLayer({
        textContentSource: await page.streamTextContent({
            includeMarkedContent: true,
            disableNormalization: true,
        }),
        container, viewport,
    })
    await textLayer.render()

    // hide "offscreen" canvases appended to docuemnt when rendering text layer
    // https://github.com/mozilla/pdf.js/blob/642b9a5ae67ef642b9a8808fd9efd447e8c350e2/web/pdf_viewer.css#L51-L58
    for (const canvas of document.querySelectorAll('.hiddenCanvasElement'))
        Object.assign(canvas.style, {
            position: 'absolute',
            top: '0',
            left: '0',
            width: '0',
            height: '0',
            display: 'none',
        })

    // fix text selection for WebKit/Chrome
    // adapted from https://github.com/mozilla/pdf.js/pull/17923
    //
    // endOfContent is the user-select:none barrier that gets moved next to
    // the selection's moving end. It lives inside the text layer (cleared
    // above), so we re-create it each render and stash it on the doc for
    // the one-time listeners below to find via doc.__foliateEndOfContent.
    const endOfContent = doc.createElement('div')
    endOfContent.className = 'endOfContent'
    container.append(endOfContent)
    doc.__foliateEndOfContent = endOfContent

    // Install the selection listeners exactly once per doc. Previously they
    // were attached on every render(), so each zoom/resize stacked another
    // generation of listeners that fought over different endOfContent
    // instances and made selection feel janky.
    if (!doc.__foliateSelectionFixInstalled) {
        doc.__foliateSelectionFixInstalled = true

        let prevRange = null

        const reset = () => {
            const eoc = doc.__foliateEndOfContent
            if (eoc) {
                container.append(eoc)
                eoc.style.width = ''
                eoc.style.height = ''
                eoc.classList.remove('active')
            }
        }

        container.addEventListener('mousedown', () => {
            doc.__foliateEndOfContent?.classList.add('active')
        })

        doc.addEventListener('pointerup', () => {
            reset()
            prevRange = null
        })

        doc.addEventListener('selectionchange', () => {
            const eoc = doc.__foliateEndOfContent
            if (!eoc) return
            const selection = doc.getSelection()
            if (selection.rangeCount === 0) {
                reset()
                return
            }

            const range = selection.getRangeAt(0)
            if (!range.intersectsNode(container)) return

            // Click-to-cancel produces a collapsed range. We must NOT
            // re-position the barrier or restyle it here — doing so
            // mid-cancel can leave the previous selection's highlight
            // visually stuck. Just reset the barrier and bail.
            if (selection.isCollapsed) {
                reset()
                return
            }

            eoc.classList.add('active')

            // Detect which boundary is the moving end. If the new range's
            // END matches the previous END (or matches the previous START,
            // for the direction-flip case), then the START is moving.
            const modifyStart = prevRange &&
                (range.compareBoundaryPoints(Range.END_TO_END, prevRange) === 0 ||
                 range.compareBoundaryPoints(Range.START_TO_END, prevRange) === 0)

            let anchor = modifyStart ? range.startContainer : range.endContainer
            if (anchor.nodeType === Node.TEXT_NODE) anchor = anchor.parentNode
            if (anchor.classList?.contains('highlight')) anchor = anchor.parentNode

            // Forward-selection edge case: when the focus has just barely
            // entered offset 0 of a new text node, the cursor is logically
            // still in the gap before it — walk back to the previous span
            // so the barrier lands behind the gap, not after the new node.
            // (mozilla/pdf.js text_layer_builder.js)
            if (!modifyStart && range.endOffset === 0) {
                do {
                    while (!anchor.previousSibling) anchor = anchor.parentNode
                    anchor = anchor.previousSibling
                } while (!anchor.childNodes.length)
            }

            if (container.contains(anchor) && anchor !== container && anchor.parentElement) {
                // Size the barrier to cover the textLayer and make it
                // selectable so the browser's drag-selection algorithm has
                // a stable place to land when the cursor is over empty
                // space — this is what kills the "selection jumps to
                // distant span" behaviour in WebKit/Chrome.
                eoc.style.width = container.style.width
                eoc.style.height = container.style.height
                eoc.style.userSelect = 'text'
                anchor.parentElement.insertBefore(
                    eoc,
                    modifyStart ? anchor : anchor.nextSibling
                )
            }

            prevRange = range.cloneRange()
        })
    }

    const div = doc.querySelector('.annotationLayer')
    const linkService = {
        goToDestination: () => {},
        getDestinationHash: dest => JSON.stringify(dest),
        addLinkAttributes: (link, url) => link.href = url,
    }
    await new pdfjsLib.AnnotationLayer({ page, viewport, div, linkService })
        .render({ annotations: await page.getAnnotations() })
}

const renderPage = async (page, getImageBlob) => {
    const viewport = page.getViewport({ scale: 1 })
    if (getImageBlob) {
        const canvas = document.createElement('canvas')
        canvas.height = viewport.height
        canvas.width = viewport.width
        const canvasContext = canvas.getContext('2d')
        await page.render({ canvasContext, viewport }).promise
        return new Promise(resolve => canvas.toBlob(resolve))
    }
    const src = URL.createObjectURL(new Blob([`
        <!DOCTYPE html>
        <html lang="en">
        <meta charset="utf-8">
        <meta name="viewport" content="width=${viewport.width}, height=${viewport.height}">
        <style>
        html, body {
            margin: 0;
            padding: 0;
        }
        /*
        https://github.com/mozilla/pdf.js/commit/bd05b255fabfc313b194bfe9a17ccded4d90fb5a
        */
        :root {
          --user-unit: 1;
          --total-scale-factor: calc(var(--scale-factor) * var(--user-unit));
          --scale-round-x: 1px;
          --scale-round-y: 1px;
        }
        /* Only the actual glyph spans inside the text layer are selectable.
         * Without this, mousedown anywhere outside a span (margins, gaps
         * between words, the canvas, the annotation layer) lands on a
         * user-select:text container, and the browser places the caret at
         * the START of that container's text content — which is the first
         * span on the page. The result is that any drag begun "a bit off"
         * a word starts the selection from the top of the page. */
        html, body, #canvas, .annotationLayer {
            -webkit-user-select: none;
            user-select: none;
        }
        .textLayer {
            -webkit-user-select: none;
            user-select: none;
        }
        .textLayer span, .textLayer br {
            -webkit-user-select: text;
            user-select: text;
        }
        ${textLayerBuilderCSS}
        /* override pdfjs selection color for consistent appearance in WebKit;
           AccentColor/color-mix can be unreliable in iframe contexts */
        .textLayer ::selection {
            background: rgba(56 117 215 / 0.25);
        }
        ${annotationLayerBuilderCSS}
        </style>
        <div id="canvas"></div>
        <div class="textLayer"></div>
        <div class="annotationLayer"></div>
    `], { type: 'text/html' }))
    let zoomTimeout = 0
    // Returns a promise that resolves once the debounced render completes.
    // Both the existing fixed-layout viewer and the new scroll viewer chain on
    // this promise to dispatch `create-overlayer` after the canvas/text layer
    // are in place.
    const onZoom = ({ doc, scale }) => new Promise(resolve => {
        clearTimeout(zoomTimeout)
        zoomTimeout = setTimeout(async () => {
            await render(page, doc, scale)
            resolve()
        }, 200)
    })
    return { src, onZoom }
}

const makeTOCItem = item => ({
    label: item.title,
    href: item.dest == null ? undefined : JSON.stringify(item.dest),
    subitems: item.items.length ? item.items.map(makeTOCItem) : null,
})

export const makePDF = async file => {
    const transport = new pdfjsLib.PDFDataRangeTransport(file.size, [])
    transport.requestDataRange = (begin, end) => {
        file.slice(begin, end).arrayBuffer().then(chunk => {
            transport.onDataRange(begin, chunk)
        })
    }
    // See the top-of-file note for why this isn't `workerSrc`. We own the
    // Worker's lifecycle because pdf.js doesn't terminate ports it didn't
    // construct itself (pdf.mjs `PDFWorker.destroy`).
    const worker = new Worker(pdfjsPath('pdf.worker.mjs'), { type: 'module' })
    pdfjsLib.GlobalWorkerOptions.workerPort = worker

    // Until `return book`, no caller has a handle to terminate the Worker:
    // `view.open()` doesn't assign `this.book = book` until `makePDF()`
    // resolves, so a throw from any await below (`getDocument`,
    // `getMetadata`, `getOutline`) would orphan the Worker. Single cleanup
    // path: tear down pdf if it exists, then terminate the Worker.
    let pdf
    try {
        pdf = await pdfjsLib.getDocument({
            range: transport,
            cMapUrl: pdfjsPath('cmaps/'),
            standardFontDataUrl: pdfjsPath('standard_fonts/'),
            isEvalSupported: false,
        }).promise

        const book = { rendition: { layout: 'pre-paginated' } }

        const { metadata, info } = await pdf.getMetadata() ?? {}
        // TODO: for better results, parse `metadata.getRaw()`
        book.metadata = {
            title: metadata?.get('dc:title') ?? info?.Title,
            author: metadata?.get('dc:creator') ?? info?.Author,
            contributor: metadata?.get('dc:contributor'),
            description: metadata?.get('dc:description') ?? info?.Subject,
            language: metadata?.get('dc:language'),
            publisher: metadata?.get('dc:publisher'),
            subject: metadata?.get('dc:subject'),
            identifier: metadata?.get('dc:identifier'),
            source: metadata?.get('dc:source'),
            rights: metadata?.get('dc:rights'),
        }

        const outline = await pdf.getOutline()
        book.toc = outline?.map(makeTOCItem)

        const cache = new Map()
        book.sections = Array.from({ length: pdf.numPages }).map((_, i) => ({
            id: i,
            load: async () => {
                const cached = cache.get(i)
                if (cached) return cached
                const url = await renderPage(await pdf.getPage(i + 1))
                cache.set(i, url)
                return url
            },
            size: 1000,
        }))

        // Expose natural page sizes without rendering — used by scroll mode to
        // reserve placeholder space before iframes are mounted.
        const sizeCache = new Map()
        book.getPageSize = async i => {
            if (sizeCache.has(i)) return sizeCache.get(i)
            const page = await pdf.getPage(i + 1)
            const { width, height } = page.getViewport({ scale: 1 })
            const size = { width, height }
            sizeCache.set(i, size)
            return size
        }
        const resolveDest = async href => {
            if (!href || href === "null") return null
            const parsed = JSON.parse(href)
            const dest = typeof parsed === 'string'
                ? await pdf.getDestination(parsed) : parsed
            if (!dest) return null
            const index = await pdf.getPageIndex(dest[0])
            const type = dest[1]?.name ?? dest[1]
            const top = type === 'XYZ' ? dest[3]
                : type === 'FitH' || type === 'FitBH' ? dest[2]
                    : type === 'FitR' ? dest[5]
                        : null
            if (typeof top !== 'number' || !Number.isFinite(top)) return { index }
            const { height } = await book.getPageSize(index)
            return { index, anchor: { top: Math.max(0, height - top) } }
        }
        book.isExternal = uri => /^\w+:/i.test(uri)
        book.resolveHref = resolveDest
        book.splitTOCHref = async href => {
            const resolved = await resolveDest(href)
            if (!resolved) return null
            return [resolved.index, null]
        }
        book.getTOCFragment = doc => doc.documentElement
        book.getCover = async () => renderPage(await pdf.getPage(1), true)
        book.destroy = () => {
            // Same shape as the setup catch path above: pdf.destroy() can
            // stall on a wedged worker, so don't await — fire-and-forget
            // and unconditionally terminate the Worker we own.
            pdf.destroy?.()?.catch?.(() => {})
            worker.terminate()
        }
        return book
    } catch (err) {
        // pdf.destroy() can stall on a wedged worker — fire-and-forget so
        // the original error propagates without being shadowed by teardown.
        // Always terminate the Worker; that's the resource we actually own.
        pdf?.destroy?.()?.catch?.(() => {})
        worker.terminate()
        throw err
    }
}
