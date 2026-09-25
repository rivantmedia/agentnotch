import { LoadingBlock, Skeleton } from "../_components/ui";

export default function SettingsLoading() {
  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-10 px-4 py-8 sm:py-10">
      <LoadingBlock label="Loading settings…" className="flex flex-col gap-8">
        <Skeleton className="h-8 w-40" />
        {[0, 1, 2].map((i) => (
          <div key={i} className="flex flex-col gap-3" aria-hidden="true">
            <Skeleton className="h-5 w-48" />
            <div className="card flex flex-col gap-3 p-5">
              <Skeleton className="h-4 w-64 max-w-full" />
              <Skeleton className="h-4 w-40" />
            </div>
          </div>
        ))}
      </LoadingBlock>
    </div>
  );
}
