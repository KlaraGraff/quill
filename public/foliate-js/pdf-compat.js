const modernWorkerUrl = new URL(
    './vendor/pdfjs/pdf.worker.mjs', import.meta.url).toString()
const legacyWorkerUrl = new URL(
    './vendor/pdfjs/legacy/pdf.worker.mjs', import.meta.url).toString()

const importModernPdfJs = () => import('./vendor/pdfjs/pdf.mjs')
const importLegacyPdfJs = () => import('./vendor/pdfjs/legacy/pdf.mjs')

export const snapshotPdfCapabilities = globalObject => ({
    urlParse: typeof globalObject?.URL?.parse === 'function',
    abortSignalAny: typeof globalObject?.AbortSignal?.any === 'function',
    promiseTry: typeof globalObject?.Promise?.try === 'function',
    promiseWithResolvers:
        typeof globalObject?.Promise?.withResolvers === 'function',
    structuredClone: typeof globalObject?.structuredClone === 'function',
    uint8ArrayFromBase64:
        typeof globalObject?.Uint8Array?.fromBase64 === 'function',
    uint8ArrayToBase64:
        typeof globalObject?.Uint8Array?.prototype?.toBase64 === 'function',
    uint8ArrayToHex:
        typeof globalObject?.Uint8Array?.prototype?.toHex === 'function',
    setIntersection:
        typeof globalObject?.Set?.prototype?.intersection === 'function',
})

export const selectPdfJsVariant = capabilities =>
    capabilities.urlParse
    && capabilities.abortSignalAny
    && capabilities.promiseTry
    && capabilities.promiseWithResolvers
    && capabilities.structuredClone
    && capabilities.uint8ArrayFromBase64
    && capabilities.uint8ArrayToBase64
    && capabilities.uint8ArrayToHex
    && capabilities.setIntersection
        ? 'modern'
        : 'legacy'

const dataCloneError = globalObject => {
    if (typeof globalObject.DOMException === 'function') {
        return new globalObject.DOMException(
            'The object could not be cloned.', 'DataCloneError')
    }
    const error = new TypeError('The object could not be cloned.')
    error.name = 'DataCloneError'
    return error
}

const createStructuredCloneFallback = globalObject => {
    const clone = (value, seen) => {
        const type = typeof value
        if (type === 'function' || type === 'symbol') {
            throw dataCloneError(globalObject)
        }
        if (value === null || type !== 'object') return value
        if (seen.has(value)) return seen.get(value)

        const tag = Object.prototype.toString.call(value)
        if (tag === '[object Date]') return new globalObject.Date(value.getTime())
        if (tag === '[object RegExp]') {
            return new globalObject.RegExp(value.source, value.flags)
        }
        if (tag === '[object ArrayBuffer]'
            || tag === '[object SharedArrayBuffer]') {
            const output = value.slice(0)
            seen.set(value, output)
            return output
        }
        if (globalObject.ArrayBuffer.isView(value)) {
            const buffer = clone(value.buffer, seen)
            const output = tag === '[object DataView]'
                ? new globalObject.DataView(
                    buffer, value.byteOffset, value.byteLength)
                : new value.constructor(buffer, value.byteOffset, value.length)
            seen.set(value, output)
            return output
        }
        if (tag === '[object Map]') {
            const output = new globalObject.Map()
            seen.set(value, output)
            for (const [key, entry] of value) {
                output.set(clone(key, seen), clone(entry, seen))
            }
            return output
        }
        if (tag === '[object Set]') {
            const output = new globalObject.Set()
            seen.set(value, output)
            for (const entry of value) output.add(clone(entry, seen))
            return output
        }
        if (tag === '[object Blob]') {
            const output = value.slice(0, value.size, value.type)
            seen.set(value, output)
            return output
        }
        if (tag === '[object File]') {
            const output = new globalObject.File([value], value.name, {
                type: value.type,
                lastModified: value.lastModified,
            })
            seen.set(value, output)
            return output
        }
        if (tag === '[object DOMException]') {
            const output = new globalObject.DOMException(value.message, value.name)
            seen.set(value, output)
            return output
        }
        if (tag.endsWith('Error]')) {
            const output = new value.constructor(value.message)
            seen.set(value, output)
            output.name = value.name
            if (value.stack) output.stack = value.stack
            if ('cause' in value) output.cause = clone(value.cause, seen)
            return output
        }
        if (tag !== '[object Array]' && tag !== '[object Object]') {
            throw dataCloneError(globalObject)
        }

        const output = tag === '[object Array]' ? [] : {}
        seen.set(value, output)
        for (const key of Object.keys(value)) output[key] = clone(value[key], seen)
        if (Array.isArray(output)) output.length = value.length
        return output
    }

    // Transfer is a PDF.js optimization. Copying preserves correctness on
    // Safari 15, where synchronous ArrayBuffer detachment cannot be polyfilled.
    return value => clone(value, new Map())
}

const ensureLegacyStructuredClone = globalObject => {
    if (typeof globalObject.structuredClone === 'function') return
    const fallback = createStructuredCloneFallback(globalObject)
    try {
        Object.defineProperty(globalObject, 'structuredClone', {
            configurable: true,
            writable: true,
            value: fallback,
        })
    } catch {
        globalObject.structuredClone = fallback
    }
}

export const createPdfJsLoader = ({
    globalObject = globalThis,
    getCapabilities = () => snapshotPdfCapabilities(globalObject),
    importModern = importModernPdfJs,
    importLegacy = importLegacyPdfJs,
    workerUrls = { modern: modernWorkerUrl, legacy: legacyWorkerUrl },
} = {}) => {
    let cachedLoad
    let selectedVariant

    const loadVariant = async variant => {
        if (variant === 'legacy') ensureLegacyStructuredClone(globalObject)
        return {
            pdfjsLib: await (variant === 'modern' ? importModern() : importLegacy()),
            workerUrl: workerUrls[variant],
            variant,
        }
    }

    const loadSelectedVariant = async () => {
        selectedVariant ??= selectPdfJsVariant(getCapabilities())
        if (selectedVariant === 'legacy') return loadVariant('legacy')

        try {
            return await loadVariant('modern')
        } catch {
            return loadVariant('legacy')
        }
    }

    return () => {
        if (cachedLoad) return cachedLoad

        const pending = loadSelectedVariant()
        const retryable = pending.catch(error => {
            if (cachedLoad === retryable) cachedLoad = undefined
            throw error
        })
        cachedLoad = retryable
        return cachedLoad
    }
}

export const loadPdfJs = createPdfJsLoader()
