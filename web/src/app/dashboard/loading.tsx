import { LoadingBlock, Skeleton } from "../_components/ui";

export default function DashboardLoading() {
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-10 px-4 py-8 sm:py-10">
      <LoadingBlock
        label="Loading the dashboard…"
        className="flex flex-col gap-8"
      >
        <div className="flex flex-col gap-2">
          <Skeleton className="h-8 w-48" />
          <Skeleton className="h-4 w-80 max-w-full" />
        </div>
        <div
          className="grid gap-4 md:grid-cols-2 xl:grid-cols-3"
          aria-hidden="true"
        >
          {[0, 1, 2].map((i) => (
            <div key={i} className="card flex flex-col gap-5 p-5">
              <Skeleton className="h-4 w-36" />
              <Skeleton className="h-2 w-full rounded-full" />
              <Skeleton className="h-2 w-full rounded-full" />
              <Skeleton className="h-9 w-full" />
            </div>
          ))}
        </div>
      </LoadingBlock>
    </div>
  );
}
