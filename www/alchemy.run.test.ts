import { describe, expect, test } from 'bun:test';
import * as Schema from 'effect/Schema';
import { stageSchema } from './alchemy.run';

const isStage = Schema.is(stageSchema);

describe('stageSchema', () => {
  test.each(['dev', 'prod', 'pr-1', 'pr-42'])('accepts %s', (stage) => {
    expect(isStage(stage)).toBe(true);
  });

  test.each(['staging', 'pr-0', 'pr--1', 'pr-1.5'])('rejects %s', (stage) => {
    expect(isStage(stage)).toBe(false);
  });
});
