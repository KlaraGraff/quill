use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ImportFormat {
    Epub,
    Pdf,
    Txt,
    Markdown,
    Html,
    Mobi,
    Fb2,
    Fbz,
    Cbz,
}

impl ImportFormat {
    pub(super) fn source_name(self) -> &'static str {
        match self {
            Self::Epub => "epub",
            Self::Pdf => "pdf",
            Self::Txt => "txt",
            Self::Markdown => "markdown",
            Self::Html => "html",
            Self::Mobi => "mobi",
            Self::Fb2 => "fb2",
            Self::Fbz => "fbz",
            Self::Cbz => "cbz",
        }
    }

    pub(super) fn native_extension(self, path: &Path) -> Option<String> {
        match self {
            Self::Mobi => path
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.to_ascii_lowercase()),
            Self::Fb2 => Some("fb2".to_string()),
            Self::Fbz => Some("fbz".to_string()),
            Self::Cbz => Some("cbz".to_string()),
            Self::Epub | Self::Pdf | Self::Txt | Self::Markdown | Self::Html => None,
        }
    }
}

pub(super) fn unsupported_format() -> AppError {
    AppError::Other("UNSUPPORTED_FORMAT".to_string())
}

pub(super) fn detect_import_format(path: &Path) -> AppResult<ImportFormat> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file()
        || metadata.len() > MAX_TEXT_IMPORT_BYTES
            && matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some(ext) if ext.eq_ignore_ascii_case("txt")
                    || ext.eq_ignore_ascii_case("md")
                    || ext.eq_ignore_ascii_case("markdown")
                    || ext.eq_ignore_ascii_case("html")
                    || ext.eq_ignore_ascii_case("htm")
            )
    {
        return Err(unsupported_format());
    }
    let mut file = fs::File::open(path)?;
    let mut header = [0_u8; 8 * 1024];
    let read = file.read(&mut header)?;
    let header = &header[..read];
    if header.starts_with(b"%PDF-") {
        return Ok(ImportFormat::Pdf);
    }
    if header.starts_with(b"PK\x03\x04") {
        let file = fs::File::open(path)?;
        let mut archive =
            ZipArchive::new(file).map_err(|_| AppError::Other("INVALID_CONTAINER".to_string()))?;
        let has_epub_mimetype = if let Ok(mut mimetype) = archive.by_name("mimetype") {
            let mut content = String::new();
            let _ = mimetype.read_to_string(&mut content);
            content.trim() == "application/epub+zip"
        } else {
            false
        };
        if has_epub_mimetype && archive.by_name("META-INF/container.xml").is_ok() {
            return Ok(ImportFormat::Epub);
        }
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
        let entries = archive
            .file_names()
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        if ext.eq_ignore_ascii_case("fbz") && entries.iter().any(|name| name.ends_with(".fb2")) {
            return Ok(ImportFormat::Fbz);
        }
        if ext.eq_ignore_ascii_case("cbz")
            && entries.iter().any(|name| {
                name.ends_with(".jpg")
                    || name.ends_with(".jpeg")
                    || name.ends_with(".png")
                    || name.ends_with(".gif")
                    || name.ends_with(".webp")
            })
        {
            return Ok(ImportFormat::Cbz);
        }
        return Err(AppError::Other("INVALID_CONTAINER".to_string()));
    }

    let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("");
    if matches!(ext.to_ascii_lowercase().as_str(), "mobi" | "azw" | "azw3")
        && header.len() >= 68
        && &header[60..68] == b"BOOKMOBI"
    {
        return Ok(ImportFormat::Mobi);
    }
    if ext.eq_ignore_ascii_case("fb2")
        && std::str::from_utf8(header)
            .ok()
            .is_some_and(|text| text.to_ascii_lowercase().contains("<fictionbook"))
    {
        return Ok(ImportFormat::Fb2);
    }
    let is_text_extension = || {
        ext.eq_ignore_ascii_case("txt")
            || ext.eq_ignore_ascii_case("md")
            || ext.eq_ignore_ascii_case("markdown")
            || ext.eq_ignore_ascii_case("html")
            || ext.eq_ignore_ascii_case("htm")
    };
    // UTF-16 text contains NUL bytes by design. Inspect the BOM before the
    // generic binary guard so decode_txt can handle UTF-16 LE/BE correctly.
    if encoding_rs::Encoding::for_bom(header).is_some() && is_text_extension() {
        return match ext.to_ascii_lowercase().as_str() {
            "txt" => Ok(ImportFormat::Txt),
            "md" | "markdown" => Ok(ImportFormat::Markdown),
            "html" | "htm" => Ok(ImportFormat::Html),
            _ => Err(unsupported_format()),
        };
    }
    if !header.contains(&0) {
        if ext.eq_ignore_ascii_case("txt") {
            return Ok(ImportFormat::Txt);
        }
        if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown") {
            return Ok(ImportFormat::Markdown);
        }
        if ext.eq_ignore_ascii_case("html") || ext.eq_ignore_ascii_case("htm") {
            return Ok(ImportFormat::Html);
        }
    }
    Err(unsupported_format())
}

pub(super) fn decode_txt(bytes: &[u8]) -> AppResult<String> {
    if bytes.len() as u64 > MAX_TEXT_IMPORT_BYTES {
        return Err(AppError::Other("TEXT_FILE_TOO_LARGE".to_string()));
    }
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        let (text, _, had_errors) = encoding.decode(&bytes[bom_len..]);
        if had_errors {
            return Err(AppError::Other("ENCODING_UNCERTAIN".to_string()));
        }
        return Ok(text.into_owned());
    }
    let mut detector = chardetng::EncodingDetector::new();
    detector.feed(bytes, true);
    let encoding = detector.guess(None, true);
    let (text, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        return Err(AppError::Other("ENCODING_UNCERTAIN".to_string()));
    }
    Ok(text.into_owned())
}

pub(super) fn source_sha256(path: &Path) -> AppResult<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(super) fn markdown_to_text(markdown: &str) -> String {
    let options = MarkdownOptions::ENABLE_STRIKETHROUGH
        | MarkdownOptions::ENABLE_TABLES
        | MarkdownOptions::ENABLE_TASKLISTS;
    let mut text = String::new();
    for event in MarkdownParser::new_ext(markdown, options) {
        match event {
            MarkdownEvent::Text(value) | MarkdownEvent::Code(value) => text.push_str(&value),
            MarkdownEvent::SoftBreak | MarkdownEvent::HardBreak => text.push('\n'),
            MarkdownEvent::End(
                MarkdownTagEnd::Paragraph
                | MarkdownTagEnd::Heading(_)
                | MarkdownTagEnd::BlockQuote(_)
                | MarkdownTagEnd::CodeBlock
                | MarkdownTagEnd::Item
                | MarkdownTagEnd::TableRow,
            ) => text.push('\n'),
            _ => {}
        }
    }
    text
}

pub(super) fn html_to_text(html: &str) -> String {
    let cleaned = ammonia::clean(html);
    scraper::Html::parse_fragment(&cleaned)
        .root_element()
        .text()
        .collect::<Vec<_>>()
        .join("\n")
}
