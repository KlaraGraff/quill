import { nodeResolve } from '@rollup/plugin-node-resolve'
import terser from '@rollup/plugin-terser'
import fsExtra from 'fs-extra'
import { parseAst } from 'rollup/parseAst'

const { copy, outputFile, readFile, remove } = fsExtra

const SAFARI15_LEGACY_RUNTIME = `
/* Lantern Safari 15 runtime compatibility for the PDF.js legacy pair. */
(() => {
    const define = (target, name, value) => Object.defineProperty(target, name, {
        configurable: true,
        writable: true,
        value,
    })
    const arrayPrototype = Array.prototype
    if (typeof arrayPrototype.at !== 'function') define(arrayPrototype, 'at', function (index) {
        if (this == null) throw new TypeError('Array.prototype.at called on null or undefined')
        const object = Object(this)
        const length = object.length >>> 0
        let relative = Number(index) || 0
        relative = relative < 0 ? Math.ceil(relative) : Math.floor(relative)
        const position = relative < 0 ? length + relative : relative
        return position < 0 || position >= length ? undefined : object[position]
    })
    if (typeof arrayPrototype.findLast !== 'function') define(arrayPrototype, 'findLast', function (callback, thisArg) {
        if (this == null) throw new TypeError('Array.prototype.findLast called on null or undefined')
        if (typeof callback !== 'function') throw new TypeError('callback must be a function')
        const object = Object(this)
        for (let index = (object.length >>> 0) - 1; index >= 0; index--) {
            if (callback.call(thisArg, object[index], index, object)) return object[index]
        }
        return undefined
    })
    if (typeof arrayPrototype.findLastIndex !== 'function') define(arrayPrototype, 'findLastIndex', function (callback, thisArg) {
        if (this == null) throw new TypeError('Array.prototype.findLastIndex called on null or undefined')
        if (typeof callback !== 'function') throw new TypeError('callback must be a function')
        const object = Object(this)
        for (let index = (object.length >>> 0) - 1; index >= 0; index--) {
            if (callback.call(thisArg, object[index], index, object)) return index
        }
        return -1
    })
    if (typeof Object.hasOwn !== 'function') define(Object, 'hasOwn', function (object, key) {
        if (object == null) throw new TypeError('Object.hasOwn called on null or undefined')
        return Object.prototype.hasOwnProperty.call(Object(object), key)
    })
})()
`

const lowerSafari15ZipCrypto = () => ({
    name: 'lower-safari15-zip-crypto',
    transform(source, id) {
        if (!id.endsWith('/core/streams/zip-crypto-stream.js')) return null
        const modern = 'decryptedHeader.at(-1)'
        if (!source.includes(modern))
            throw new Error('zip.js ZipCrypto compatibility target changed')
        return {
            code: source.replace(
                modern,
                'decryptedHeader[decryptedHeader.length - 1]'),
            map: null,
        }
    },
})

const findStaticBlocks = node => {
    if (!node || typeof node !== 'object') return []
    if (Array.isArray(node)) return node.flatMap(findStaticBlocks)
    return [
        ...(node.type === 'StaticBlock' ? [node] : []),
        ...Object.values(node).flatMap(findStaticBlocks),
    ]
}

const lowerStaticBlocks = source => findStaticBlocks(parseAst(source))
    .sort((a, b) => b.start - a.start)
    .reduce((output, block, index) => {
        const bodyStart = source.indexOf('{', block.start) + 1
        const body = source.slice(bodyStart, block.end - 1)
        const replacement =
            `static #pdfjsSafari15Initializer${index} = (() => {${body}})();`
        return output.slice(0, block.start) + replacement + output.slice(block.end)
    }, source)

const copyLegacyPDFJS = async (sourcePath, outputPath) => {
    const source = await readFile(sourcePath, 'utf8')
    const withoutSourceMap = source.replace(
        /\n\/\/# sourceMappingURL=[^\r\n]+(?:\r?\n)?$/, '\n')
    await outputFile(outputPath, SAFARI15_LEGACY_RUNTIME + lowerStaticBlocks(withoutSourceMap))
}

const copyPDFJS = () => ({
    name: 'copy-pdfjs',
    async writeBundle() {
        await remove('vendor/pdfjs/legacy')
        await Promise.all([
            copy('node_modules/pdfjs-dist/build/pdf.mjs', 'vendor/pdfjs/pdf.mjs'),
            copy('node_modules/pdfjs-dist/build/pdf.mjs.map', 'vendor/pdfjs/pdf.mjs.map'),
            copy('node_modules/pdfjs-dist/build/pdf.worker.mjs', 'vendor/pdfjs/pdf.worker.mjs'),
            copy('node_modules/pdfjs-dist/build/pdf.worker.mjs.map', 'vendor/pdfjs/pdf.worker.mjs.map'),
            copyLegacyPDFJS(
                'node_modules/pdfjs-dist/legacy/build/pdf.mjs',
                'vendor/pdfjs/legacy/pdf.mjs'),
            copyLegacyPDFJS(
                'node_modules/pdfjs-dist/legacy/build/pdf.worker.mjs',
                'vendor/pdfjs/legacy/pdf.worker.mjs'),
            copy('node_modules/pdfjs-dist/cmaps', 'vendor/pdfjs/cmaps'),
            copy('node_modules/pdfjs-dist/standard_fonts', 'vendor/pdfjs/standard_fonts'),
        ])
    },
})

export default [{
    input: 'rollup/fflate.js',
    output: {
        dir: 'vendor/',
        format: 'esm',
    },
    plugins: [nodeResolve(), terser()],
},
{
    input: 'rollup/zip.js',
    output: {
        dir: 'vendor/',
        format: 'esm',
    },
    plugins: [nodeResolve(), lowerSafari15ZipCrypto(), terser(), copyPDFJS()],
}]
