import { useEffect, useState } from "react";
import { KeyRound } from "lucide-react";
import { toast } from "sonner";
import { Hint } from "@/components/hint";
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
import { getBearer, setBearer } from "@/lib/auth";

export function BearerDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const [value, setValue] = useState("");

  useEffect(() => {
    if (open) {
      setValue(getBearer() ?? "");
    }
  }, [open]);

  function save() {
    setBearer(value);
    onOpenChange(false);
    toast.success(value.trim() ? "Bearer stored in this browser" : "Bearer cleared");
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Bearer token</DialogTitle>
          <DialogDescription>
            Sent as <code>Authorization: Bearer</code> on <code>POST /sql</code> and
            as <code>?token=</code> on <code>/ws</code>. Same env the server reads:{" "}
            <code>AIDB_BEARER</code>. No users table.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-1.5">
          <Label htmlFor="aidb-bearer">Token</Label>
          <Input
            id="aidb-bearer"
            type="password"
            autoComplete="off"
            value={value}
            onChange={(event) => setValue(event.target.value)}
            placeholder="AIDB_BEARER"
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                save();
              }
            }}
          />
        </div>
        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => {
              setBearer(null);
              onOpenChange(false);
              toast.success("Bearer cleared");
            }}
          >
            Clear
          </Button>
          <Button onClick={save}>Save</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

export function BearerButton({
  locked,
  onClick,
}: {
  locked?: boolean;
  onClick: () => void;
}) {
  return (
    <Hint label={locked ? "Bearer required — click to set" : "Set AIDB_BEARER for this browser"}>
      <Button
        variant={locked ? "destructive" : "ghost"}
        size="icon-sm"
        aria-label="Bearer token"
        onClick={onClick}
      >
        <KeyRound />
      </Button>
    </Hint>
  );
}
