import { useEffect, useState } from "react";
import { Send } from "lucide-react";
import { Banner, DeleteButton, Field, Header, NotYet } from "@/components/common";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { getMeeting, NEWER_VERSION, SCHEMA_VERSION, type MeetingDetail } from "@/lib/ipc";

// LLM output is untrusted: title and summary are plain text in form fields, never HTML (D6, NFR-07).
// ponytail: read-only until saving lands in M4 (`save_summary`).
export default function Review({ id, onHome }: { id: string; onHome: () => void }) {
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    getMeeting(id)
      .then(setDetail)
      .catch((e) => setError(String(e)));
  }, [id]);

  const summary = detail?.summary ?? "";

  return (
    <>
      <Header title="검토">
        <Button variant="outline" onClick={onHome}>
          홈으로
        </Button>
      </Header>
      <main className="mx-auto max-w-2xl space-y-4 p-6">
        {error && <Banner kind="error">{error}</Banner>}
        {detail && detail.meta.schema_version > SCHEMA_VERSION && (
          <Banner kind="warning">{NEWER_VERSION}</Banner>
        )}
        <p className="text-muted-foreground">제목과 요약 편집은 준비 중입니다.</p>
        <Field label="제목">
          <Input value={detail?.meta.title ?? ""} readOnly />
        </Field>
        <Field label="요약">
          <Textarea className="h-96 field-sizing-fixed font-mono" value={summary} readOnly />
        </Field>
        <p className="text-right text-sm text-muted-foreground">
          {Array.from(summary).length.toLocaleString("ko-KR")}자
        </p>
        <div className="flex items-center gap-2">
          <NotYet hint="전송 기능 준비 중">
            <Button disabled>
              <Send aria-hidden />
              Slack으로 전송
            </Button>
          </NotYet>
          <DeleteButton id={id} onDeleted={onHome} onError={setError} />
        </div>
      </main>
    </>
  );
}
