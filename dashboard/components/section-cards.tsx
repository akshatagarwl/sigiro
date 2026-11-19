import { IconTrendingDown, IconTrendingUp } from "@tabler/icons-react"

import { Badge } from "@/components/ui/badge"
import {
  Card,
  CardAction,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"

export type SectionCard = {
  label: string
  value: string
  /** Change against the previous window, in percent. */
  change?: number
  /** Whether a rise is bad (errors) rather than neutral (traffic). */
  riseIsBad?: boolean
  headline: string
  note: string
}

/** Telemetry summaries using the SDK's shared monochrome surface tokens. */
export function SectionCards({ items }: { items: SectionCard[] }) {
  return (
    <div className="grid grid-cols-1 gap-0 px-4 lg:px-6 @xl/main:grid-cols-2 @5xl/main:grid-cols-5">
      {items.map((item) => {
        const up = (item.change ?? 0) >= 0
        const Trend = up ? IconTrendingUp : IconTrendingDown
        return (
          <Card key={item.label} className="@container/card gap-3" data-kpi={item.label}>
            <CardHeader>
              <CardDescription>{item.label}</CardDescription>
              <CardTitle className="text-2xl font-semibold tabular-nums @[250px]/card:text-3xl">
                {item.value}
              </CardTitle>
              {item.change !== undefined && Number.isFinite(item.change) && (
                <CardAction>
                  <Badge
                    variant="outline"
                    className={
                      item.riseIsBad && up && item.change > 0
                        ? "text-destructive"
                        : undefined
                    }
                  >
                    <Trend />
                    {up ? "+" : ""}
                    {item.change.toFixed(1)}%
                  </Badge>
                </CardAction>
              )}
            </CardHeader>
            <CardFooter className="flex-col items-start gap-1 text-xs">
              <div className="line-clamp-1 flex gap-2 font-medium">
                {item.headline}
              </div>
              <div className="text-muted-foreground">{item.note}</div>
            </CardFooter>
          </Card>
        )
      })}
    </div>
  )
}
