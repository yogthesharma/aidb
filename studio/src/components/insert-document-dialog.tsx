import { useState } from "react";
import { Loader2 } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { cellText, runSql, sqlString } from "@/lib/aidb";

export function InsertDocumentDialog({
  open,
  onOpenChange,
  onInserted,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onInserted: (id: string) => void;
}) {
  const [title, setTitle] = useState("");
  const [content, setContent] = useState("");
  const [saving, setSaving] = useState(false);

  async function submit() {
    const nextTitle = title.trim();
    const nextContent = content.trim();
    if (!nextTitle || !nextContent) {
      toast.error("Title and content are required");
      return;
    }
    setSaving(true);
    try {
      const result = await runSql(
        `SELECT aidb_insert_document(${sqlString(nextTitle)}, ${sqlString(nextContent)}, '{}')`,
      );
      if (!result.ok) {
        toast.error(result.error ?? "insert failed");
        return;
      }
      const id = result.rows?.[0] ? cellText(result.rows[0][0]) : "";
      toast.success("Document inserted", {
        description: id && id !== "NULL" ? id : undefined,
      });
      setTitle("");
      setContent("");
      onOpenChange(false);
      if (id && id !== "NULL") {
        onInserted(id);
      }
    } catch (err) {
      toast.error(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>Add document</DialogTitle>
          <DialogDescription>
            Same SQL as the CLI: <code>aidb_insert_document</code>. Indexing
            happens in this file.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-3">
          <div className="grid gap-1.5">
            <Label htmlFor="doc-title">Title</Label>
            <Input
              id="doc-title"
              value={title}
              onChange={(event) => setTitle(event.target.value)}
              placeholder="Refunds"
            />
          </div>
          <div className="grid gap-1.5">
            <Label htmlFor="doc-content">Content</Label>
            <Textarea
              id="doc-content"
              value={content}
              onChange={(event) => setContent(event.target.value)}
              placeholder="Refunds are issued within 14 days of purchase."
              className="min-h-32"
            />
          </div>
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={() => void submit()} disabled={saving}>
            {saving ? <Loader2 className="animate-spin" /> : null}
            Insert
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
