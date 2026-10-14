import { useEffect, useState } from "react";
import { Banner, DeleteButton, Header } from "@/components/common";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { getMeeting, NEWER_VERSION, SCHEMA_VERSION, STATUS_LABEL, type Meta } from "@/lib/ipc";

export default function Processing({ id, onHome }: { id: string; onHome: () => void }) {
  const [meta, setMeta] = useState<Meta | null>(null);
  const [error, setError] = useState("");

  useEffect(() => {
    getMeeting(id)
      .then((detail) => setMeta(detail.meta))
      .catch((e) => setError(String(e)));
  }, [id]);

  return (
    <>
      <Header title="처리 중">
        <Button variant="outline" onClick={onHome}>
          홈으로
        </Button>
      </Header>
      <main className="mx-auto max-w-2xl space-y-6 p-6">
        {error && <Banner kind="error">{error}</Banner>}
        {meta && (
          <>
            {meta.schema_version > SCHEMA_VERSION && <Banner kind="warning">{NEWER_VERSION}</Banner>}
            <Card>
              <CardHeader>
                <CardTitle>
                  <h2 className="text-xl font-bold">{meta.title || "제목 없음"}</h2>
                </CardTitle>
              </CardHeader>
              <CardContent>
                <dl className="grid grid-cols-[auto_1fr] items-center gap-x-6 gap-y-2 text-base">
                  <dt className="text-muted-foreground">상태</dt>
                  <dd>
                    <Badge variant="secondary">{STATUS_LABEL[meta.status] ?? meta.status}</Badge>
                  </dd>
                  <dt className="text-muted-foreground">녹음 시각</dt>
                  <dd>{new Date(meta.created_at).toLocaleString("ko-KR")}</dd>
                  <dt className="text-muted-foreground">전사한 구간</dt>
                  <dd>{meta.chunks_done}개</dd>
                </dl>
              </CardContent>
            </Card>
            {meta.error && <Banner kind="error">{meta.error}</Banner>}
            <DeleteButton id={id} onDeleted={onHome} onError={setError} />
          </>
        )}
      </main>
    </>
  );
}
