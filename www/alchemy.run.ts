import * as Alchemy from 'alchemy';
import * as Cloudflare from 'alchemy/Cloudflare';
import * as Effect from 'effect/Effect';
import * as Schema from 'effect/Schema';

export const stageSchema = Schema.Union([
  Schema.Literals(['dev', 'prod']),
  Schema.TemplateLiteral([
    'pr-',
    Schema.NumberFromString.pipe(
      Schema.check(Schema.isInt()),
      Schema.check(Schema.isGreaterThan(0)),
    ),
  ]),
]);

export default Alchemy.Stack(
  'SigiroWebsite',
  {
    providers: Cloudflare.providers(),
    state: Cloudflare.state(),
  },
  Effect.gen(function* () {
    const stage = yield* Schema.decodeUnknownEffect(stageSchema)((yield* Alchemy.Stack).stage).pipe(
      Effect.orDie,
    );
    return { stage };
  }),
);
