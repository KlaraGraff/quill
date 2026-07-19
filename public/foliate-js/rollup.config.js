import { nodeResolve } from '@rollup/plugin-node-resolve'
import terser from '@rollup/plugin-terser'
import fsExtra from 'fs-extra'
import { parseAst } from 'rollup/parseAst'

const { copy, outputFile, readFile, remove } = fsExtra

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
    await outputFile(outputPath, lowerStaticBlocks(withoutSourceMap))
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
    plugins: [nodeResolve(), terser(), copyPDFJS()],
}]
