import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { open } from "@tauri-apps/plugin-dialog";
import { convertFileSrc } from "@tauri-apps/api/core";
import { ImagePlus, Loader2 } from "lucide-react";
import Button from "./ui/Button";
import Input from "./ui/Input";
import { updateBookCover, updateBookMetadata } from "../hooks/useBooks";

interface EditMetadataModalProps {
  bookId: string;
  currentTitle: string;
  currentAuthor: string;
  currentCover?: string | null;
  onClose: () => void;
  onSaved: () => void;
}

export default function EditMetadataModal({
  bookId,
  currentTitle,
  currentAuthor,
  currentCover,
  onClose,
  onSaved,
}: EditMetadataModalProps) {
  const { t } = useTranslation();
  const [title, setTitle] = useState(currentTitle);
  const [author, setAuthor] = useState(currentAuthor);
  const [saving, setSaving] = useState(false);
  const [coverPath, setCoverPath] = useState<string | null>(null);
  const [coverPreview, setCoverPreview] = useState<string | null>(currentCover ?? null);
  const [error, setError] = useState<string | null>(null);

  const trimmedTitle = title.trim();
  const unchanged =
    trimmedTitle === currentTitle && author.trim() === currentAuthor && !coverPath;
  const canSave = trimmedTitle.length > 0 && !unchanged && !saving;

  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handleKey);
    return () => document.removeEventListener("keydown", handleKey);
  }, [onClose]);

  const handleSave = async () => {
    if (!canSave) return;
    setSaving(true);
    setError(null);
    try {
      if (trimmedTitle !== currentTitle || author.trim() !== currentAuthor) {
        await updateBookMetadata(bookId, trimmedTitle, author.trim());
      }
      if (coverPath) await updateBookCover(bookId, coverPath);
      onSaved();
    } catch (err) {
      console.error("Failed to update metadata:", err);
      setError(t("editInfo.saveFailed"));
    } finally {
      setSaving(false);
    }
  };

  const chooseCover = async () => {
    const selected = await open({
      multiple: false,
      directory: false,
      filters: [{ name: t("editInfo.cover"), extensions: ["jpg", "jpeg", "png", "webp"] }],
    });
    if (!selected || Array.isArray(selected)) return;
    setCoverPath(selected);
    setCoverPreview(convertFileSrc(selected));
  };

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-overlay"
      onClick={(e) => e.target === e.currentTarget && onClose()}
    >
      <div className="bg-bg-surface rounded-xl shadow-lg w-[400px] p-6">
        <h3 className="text-[18px] font-semibold text-text-primary mb-5">
          {t("editInfo.title")}
        </h3>

        <div className="flex flex-col gap-4">
          <div className="flex items-center gap-4">
            <div className="flex h-[132px] w-[92px] shrink-0 items-center justify-center overflow-hidden rounded-md border border-border bg-bg-muted">
              {coverPreview ? (
                <img src={coverPreview} alt="" className="h-full w-full object-cover" />
              ) : (
                <ImagePlus size={22} className="text-text-muted" />
              )}
            </div>
            <div className="min-w-0">
              <p className="mb-2 text-[13px] font-medium text-text-secondary">{t("editInfo.cover")}</p>
              <Button variant="secondary" size="sm" onClick={() => { void chooseCover(); }}>
                <ImagePlus size={14} />
                {t("editInfo.changeCover")}
              </Button>
              <p className="mt-2 text-[11px] leading-4 text-text-muted">{t("editInfo.coverHint")}</p>
            </div>
          </div>
          <div>
            <label className="block text-[13px] font-medium text-text-secondary mb-1.5">
              {t("editInfo.bookTitle")}
            </label>
            <Input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              autoFocus
            />
          </div>
          <div>
            <label className="block text-[13px] font-medium text-text-secondary mb-1.5">
              {t("editInfo.bookAuthor")}
            </label>
            <Input
              value={author}
              onChange={(e) => setAuthor(e.target.value)}
            />
          </div>
        </div>

        {error && <p role="alert" className="mt-3 text-[12px] text-danger-text">{error}</p>}

        <div className="flex justify-end gap-3 mt-6">
          <Button variant="ghost" size="md" onClick={onClose}>
            {t("editInfo.cancel")}
          </Button>
          <Button
            variant="primary"
            size="md"
            onClick={handleSave}
            disabled={!canSave}
          >
            {saving && <Loader2 size={14} className="animate-spin" />}
            {t("editInfo.save")}
          </Button>
        </div>
      </div>
    </div>
  );
}
