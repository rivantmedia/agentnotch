import { LoadingBlock, Skeleton } from "../../_components/ui";

export default function ProjectsLoading() {
  return (
    <div className="mx-auto flex max-w-6xl flex-col gap-8 px-4 py-8 sm:py-10">
      <LoadingBlock
        label="Loading your projects…"
        className="flex flex-col gap-8"
      >
        <div className="flex flex-col gap-2">
          <Skeleton className="h-8 w-40" />
          <Skeleton className="h-4 w-96 max-w-full" />
        </div>
        <div className="card flex flex-col gap-5 p-5" aria-hidden="true">
          {[0, 1, 2, 3].map((i) => (
            <div key={i} className="flex flex-col gap-2">
              <Skeleton className="h-4 w-48 max-w-1/2" />
              <Skeleton className="h-2 w-full rounded-full" />
            </div>
          ))}
        </div>
      </LoadingBlock>
    </div>
  );
}
