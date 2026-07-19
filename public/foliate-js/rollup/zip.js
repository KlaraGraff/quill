import { Inflate } from 'fflate'
import {
    configure,
    ZipReader,
    BlobReader,
    TextWriter,
    BlobWriter,
} from '../node_modules/@zip.js/zip.js/lib/zip-core-base.js'

class FflateDecompressionStream {
    constructor(format) {
        if (format !== 'deflate-raw')
            throw new TypeError(`Unsupported compression format: ${format}`)

        let controller
        const inflate = new Inflate(chunk => {
            if (chunk.length) controller.enqueue(chunk)
        })
        const stream = new TransformStream({
            start(value) {
                controller = value
            },
            transform(chunk) {
                inflate.push(chunk, false)
            },
            flush() {
                inflate.push(new Uint8Array(), true)
            },
        })
        this.readable = stream.readable
        this.writable = stream.writable
    }
}

configure({ DecompressionStreamZlib: FflateDecompressionStream })

export { configure, ZipReader, BlobReader, TextWriter, BlobWriter }
