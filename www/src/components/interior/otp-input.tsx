"use client";

import { OTPInput as NativeOtpInput, type SlotProps } from "input-otp";
import { AnimatePresence, motion, useReducedMotion } from "motion/react";
import {
  useId,
  useImperativeHandle,
  useRef,
  useState,
  type ReactElement,
  type Ref,
} from "react";

const CROSSFADE = {
  type: "spring",
  stiffness: 260,
  damping: 34,
  mass: 0.8,
} as const;
const EASE = [0.23, 1, 0.32, 1] as const;

export type OtpMode = "numeric" | "alphanumeric";
export type OtpStatus = "idle" | "error" | "success";

export type OtpInputHandle = {
  clear: () => void;
  focus: () => void;
  select: () => void;
};

export type OtpInputProps = {
  length?: number;
  mode?: OtpMode;
  defaultValue?: string;
  onChange?: (value: string) => void;
  onComplete?: (value: string) => void;
  status?: OtpStatus;
  errorMessage?: string;
  successMessage?: string;
  hint?: string;
  label?: string;
  groupEvery?: number;
  disabled?: boolean;
  autoFocus?: boolean;
  className?: string;
  ref?: Ref<OtpInputHandle>;
};

const allowed = (mode: OtpMode): RegExp =>
  mode === "numeric" ? /^[0-9]*$/ : /^[0-9a-zA-Z]*$/;

const sanitize = (value: string, mode: OtpMode, length: number): string => {
  const pattern = mode === "numeric" ? /[^0-9]/g : /[^0-9a-zA-Z]/g;
  return value.replace(pattern, "").slice(0, length);
};

interface CellProps extends SlotProps {
  readonly index: number;
  readonly length: number;
  readonly error: boolean;
  readonly success: boolean;
  readonly disabled: boolean;
  readonly gap: boolean;
  readonly reduced: boolean;
}

function Cell({
  index,
  length,
  char,
  isActive,
  hasFakeCaret,
  error,
  success,
  disabled,
  gap,
  reduced,
}: CellProps): ReactElement {
  const enter = reduced ? { duration: 0 } : { duration: 0.22, ease: EASE };

  return (
    <div
      aria-hidden="true"
      data-slot={index + 1}
      data-slot-count={length}
      className={`relative h-11 w-8 shrink-0 rounded-[10px] border-2 transition-[background-color,border-color,box-shadow] duration-150 sm:h-12 sm:w-10 ${
        gap ? "ml-1 sm:ml-3" : ""
      } ${
        error
          ? "border-red-500 bg-white dark:border-red-400 dark:bg-[#252522]"
          : success
            ? "border-emerald-500 bg-white dark:border-emerald-400 dark:bg-[#252522]"
            : isActive
              ? "border-[#4568FF] bg-white dark:border-[#93B0FF] dark:bg-[#252522]"
              : char
                ? "border-stone-300 bg-white dark:border-white/20 dark:bg-[#252522]"
                : "border-stone-200 bg-stone-100/70 shadow-[inset_0_1px_2px_rgba(28,25,23,0.07)] dark:border-white/[0.08] dark:bg-[#1D1D1A] dark:shadow-[inset_0_1px_2px_rgba(0,0,0,0.45)]"
      } ${disabled ? "opacity-50" : ""}`}
    >
      <span className="pointer-events-none absolute inset-0 grid place-items-center">
        <AnimatePresence initial={false} mode="popLayout">
          {char ? (
            <motion.span
              key={`${index}-${char}`}
              initial={
                reduced
                  ? false
                  : { opacity: 0, scale: 0.97, y: 10, filter: "blur(6px)" }
              }
              animate={{ opacity: 1, scale: 1, y: 0, filter: "blur(0px)" }}
              exit={
                reduced
                  ? { opacity: 0 }
                  : { opacity: 0, scale: 0.98, y: -6, filter: "blur(3px)" }
              }
              transition={enter}
              className="col-start-1 row-start-1 font-mono text-[15px] text-stone-700 tabular-nums dark:text-stone-200"
            >
              {char}
            </motion.span>
          ) : null}
        </AnimatePresence>

        {hasFakeCaret && !char && !disabled ? (
          <motion.span
            className="col-start-1 row-start-1 block h-[17px] w-[1.5px] rounded-[1px] bg-stone-700 dark:bg-stone-200"
            initial={{ opacity: 1 }}
            animate={reduced ? { opacity: 1 } : { opacity: [1, 1, 0, 0] }}
            transition={
              reduced
                ? { duration: 0 }
                : {
                    duration: 1.06,
                    times: [0, 0.5, 0.5, 1],
                    repeat: Infinity,
                    ease: "linear",
                  }
            }
          />
        ) : null}
      </span>
    </div>
  );
}

export function OtpInput({
  length = 6,
  mode = "numeric",
  defaultValue = "",
  onChange,
  onComplete,
  status = "idle",
  errorMessage = "",
  successMessage = "",
  hint = "",
  label = "Verification code",
  groupEvery = 3,
  disabled = false,
  autoFocus = false,
  className = "",
  ref,
}: OtpInputProps): ReactElement {
  const reduced = useReducedMotion();
  const statusId = useId();
  const input = useRef<HTMLInputElement>(null);
  const [value, setValue] = useState(() =>
    sanitize(defaultValue, mode, length)
  );
  const error = status === "error";
  const success = status === "success";
  useImperativeHandle(
    ref,
    () => ({
      clear: () => {
        setValue("");
        onChange?.("");
        input.current?.focus();
      },
      focus: () => input.current?.focus(),
      select: () => {
        input.current?.focus();
        input.current?.select();
      },
    }),
    [onChange]
  );

  const message = error ? errorMessage : success ? successMessage : hint;
  const hasMessage = message.length > 0;
  const messageTone = error
    ? "text-red-600 dark:text-red-400"
    : success
      ? "text-emerald-600 dark:text-emerald-400"
      : "text-stone-500 dark:text-stone-400";
  const swap = reduced ? { duration: 0 } : CROSSFADE;

  return (
    <div className={`inline-flex flex-col ${className}`}>
      <NativeOtpInput
        ref={input}
        name="verification-code"
        value={value}
        maxLength={length}
        pattern={allowed(mode).source}
        inputMode={mode === "numeric" ? "numeric" : "text"}
        autoComplete="one-time-code"
        autoFocus={autoFocus}
        disabled={disabled}
        aria-label={label}
        aria-invalid={error || undefined}
        aria-describedby={hasMessage ? statusId : undefined}
        pushPasswordManagerStrategy="increase-width"
        pasteTransformer={(pasted) => sanitize(pasted, mode, length)}
        containerClassName="relative inline-flex"
        onChange={(next) => {
          const clean = sanitize(next, mode, length);
          const newlyCompleted = clean.length === length && clean !== value;
          setValue(clean);
          onChange?.(clean);
          if (newlyCompleted) onComplete?.(clean);
        }}
        render={({ slots }) => (
          <motion.div
            className="relative flex gap-1 sm:gap-2"
            initial={false}
            variants={{ idle: { x: 0 }, wrong: { x: [0, -5, 4, -3, 0] } }}
            animate={error && !reduced ? "wrong" : "idle"}
            transition={{ duration: 0.32, ease: EASE }}
          >
            {slots.map((slot, index) => (
              <Cell
                key={index}
                {...slot}
                index={index}
                length={length}
                error={error}
                success={success}
                disabled={disabled}
                gap={groupEvery > 0 && index > 0 && index % groupEvery === 0}
                reduced={Boolean(reduced)}
              />
            ))}
          </motion.div>
        )}
      />

      {hasMessage && (
        <div
          id={statusId}
          aria-live="polite"
          className="mt-2 grid min-h-4 text-[11.5px] leading-[16px]"
        >
          <AnimatePresence initial={false} mode="wait">
            <motion.p
              key={status}
              initial={reduced ? { opacity: 0 } : { opacity: 0, y: 3 }}
              animate={{ opacity: 1, y: 0 }}
              exit={reduced ? { opacity: 0 } : { opacity: 0, y: -3 }}
              transition={swap}
              className={`col-start-1 row-start-1 ${messageTone}`}
            >
              {message}
            </motion.p>
          </AnimatePresence>
        </div>
      )}
    </div>
  );
}
