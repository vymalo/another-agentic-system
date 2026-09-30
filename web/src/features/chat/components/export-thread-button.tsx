import { DownloadIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { ThreadExporter } from "@/features/chat/hooks/use-export-thread";

/**
 * "Export JSON": downloads the whole thread as a file to send to a developer. Disabled until the
 * thread is known and while a download is being fetched.
 */
export function ExportThreadButton({
  exporter,
  disabled,
}: {
  exporter: ThreadExporter;
  disabled: boolean;
}) {
  return (
    <Button
      type="button"
      variant="outline"
      size="sm"
      disabled={disabled || exporter.exporting}
      onClick={exporter.run}
      title="Download this chat as a JSON file, to share with a developer"
    >
      <DownloadIcon aria-hidden="true" />
      {exporter.exporting ? "Exporting…" : "Export JSON"}
    </Button>
  );
}
