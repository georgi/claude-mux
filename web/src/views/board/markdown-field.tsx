import { useState } from "react";
import { Label } from "@/components/ui/label";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { Markdown } from "../lazy";

interface Props {
  id: string;
  label: string;
  value: string;
  onChange: (text: string) => void;
  placeholder?: string;
  rows?: number;
  disabled?: boolean;
}

/** A Markdown textarea with a Write / Preview tab. */
export function MarkdownField({ id, label, value, onChange, placeholder, rows = 6, disabled }: Props) {
  const [tab, setTab] = useState<"write" | "preview">("write");
  return (
    <Tabs value={tab} onValueChange={(v) => setTab(v as "write" | "preview")} className="gap-1.5">
      <div className="flex items-center justify-between gap-2">
        <Label htmlFor={id}>{label}</Label>
        <TabsList aria-label="Write or preview">
          <TabsTrigger value="write">Write</TabsTrigger>
          <TabsTrigger value="preview">Preview</TabsTrigger>
        </TabsList>
      </div>
      <TabsContent value="write">
        <Textarea id={id} rows={rows} value={value} placeholder={placeholder} disabled={disabled} onChange={(e) => onChange(e.target.value)} />
      </TabsContent>
      <TabsContent value="preview" className="min-h-24 rounded-md border px-3 py-2" aria-label="Preview">
        {value.trim() ? <Markdown>{value}</Markdown> : <p className="text-muted-foreground">Nothing to preview.</p>}
      </TabsContent>
    </Tabs>
  );
}
