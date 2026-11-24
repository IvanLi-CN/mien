import { useParams } from "react-router";
import { Surface } from "../components/surface";

/** 详情与编辑共用一个组件：编辑路由只换文案，内容由 T9-E 分批填充。 */
export function ProfileDetailPage({ edit = false }: { edit?: boolean }) {
  const { id } = useParams<{ id: string }>();
  return (
    <Surface
      titleKey={edit ? "pages.profile_edit" : "pages.profile_detail"}
      hintKey={edit ? "hints.profile_edit" : "hints.profile_detail"}
      task={edit ? "T9-A / T9-E" : "T9-A / T9-D / T9-E"}
    >
      <p className="font-mono text-muted text-sm">{id}</p>
    </Surface>
  );
}
