import { useTranslation } from "react-i18next";
import { Link } from "react-router";

export function NotFoundPage() {
  const { t } = useTranslation();
  return (
    <section className="flex flex-col gap-2">
      <h1 className="font-medium text-lg">{t("pages.not_found")}</h1>
      <Link className="text-accent text-sm" to="/profiles">
        {t("pages.back")}
      </Link>
    </section>
  );
}
