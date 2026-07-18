"""OCRmyPDF plugin that emits Lantern's stable JSONL progress contract."""

from __future__ import annotations

import json
import logging
import os
import threading
from typing import Any

from ocrmypdf import hookimpl


_write_lock = threading.Lock()
_stats_lock = threading.Lock()
_stats: dict[str, int] = {
    "pages": 0,
    "ocr_pages": 0,
    "skipped_pages": 0,
    "timed_out_pages": 0,
}


def _write_json(payload: dict[str, Any]) -> None:
    encoded = json.dumps(payload, ensure_ascii=True, separators=(",", ":")) + "\n"
    from ocrmypdf._stdoutprotect import get_protected_stdout_fd

    output = get_protected_stdout_fd()
    with _write_lock:
        os.write(output if output is not None else 1, encoded.encode("utf-8"))


def reset_stats() -> None:
    with _stats_lock:
        _stats.update(pages=0, ocr_pages=0, skipped_pages=0, timed_out_pages=0)


def complete_payload() -> dict[str, int | str]:
    with _stats_lock:
        return {"type": "complete", **_stats}


class JsonlProgressBar:
    def __init__(
        self,
        *,
        total: int | float | None = None,
        desc: str | None = None,
        unit: str | None = None,
        disable: bool = False,
        **_kwargs: Any,
    ) -> None:
        self.total = total
        self.desc = desc or ""
        self.unit = unit or ""
        self.disable = disable
        self.current = 0.0
        self.phase = _phase_for(self.desc)

    def __enter__(self) -> "JsonlProgressBar":
        if not self.disable:
            _write_json({"type": "phase", "phase": self.phase})
        return self

    def __exit__(self, _exc_type: object, _exc: object, _traceback: object) -> bool:
        return False

    def update(self, n: float = 1, *, completed: float | None = None) -> None:
        self.current = float(completed) if completed is not None else self.current + float(n or 0)
        if self.disable or self.total is None or self.unit != "page":
            return
        _write_json(
            {
                "type": "progress",
                "phase": self.phase,
                "completed": int(self.current),
                "total": int(float(self.total)),
            }
        )


class JsonlConsoleHandler(logging.StreamHandler):
    def emit(self, record: logging.LogRecord) -> None:
        message = record.getMessage()
        if "took too long to OCR" in message:
            page = getattr(record, "pageno", None)
            payload: dict[str, int | str] = {"type": "warning", "code": "PAGE_TIMEOUT"}
            if isinstance(page, int):
                payload["page"] = page
            elif isinstance(page, str) and page.strip().isdigit():
                payload["page"] = int(page.strip())
            with _stats_lock:
                _stats["timed_out_pages"] += 1
            _write_json(payload)
        super().emit(record)


def _phase_for(description: str) -> str:
    normalized = description.casefold()
    if "scanning" in normalized or "analy" in normalized:
        return "analyzing"
    if normalized == "ocr" or "image processing" in normalized:
        return "ocr"
    return "finalizing"


@hookimpl
def get_progressbar_class() -> type[JsonlProgressBar]:
    return JsonlProgressBar


@hookimpl
def get_logging_console() -> logging.Handler:
    return JsonlConsoleHandler()


@hookimpl
def check_options(options: Any) -> None:
    options.progress_bar = True


@hookimpl
def validate(pdfinfo: Any, options: Any) -> None:
    pages = list(pdfinfo)
    with _stats_lock:
        _stats["pages"] = len(pages)
        _stats["skipped_pages"] = sum(1 for page in pages if page.has_text)
        _stats["ocr_pages"] = len(pages) - _stats["skipped_pages"]
