/**
 * Meerkat mascot — sigiro's investigator.
 * Poses: standing (default), looking, peeking, magnifying
 * All SVG, scales to any size via width/height props.
 */

type Pose = 'standing' | 'looking' | 'peeking' | 'magnifying'

interface MeerkatProps {
  pose?: Pose
  size?: number
  className?: string
  style?: React.CSSProperties
}

// Warm meerkat color palette
const C = {
  body: '#D4A574',       // warm tan
  belly: '#F0D9B5',      // light cream belly
  face: '#E8C89E',       // lighter face
  nose: '#3D2B1F',       // dark brown nose
  eyes: '#1A1A1A',       // near-black eyes
  eyeWhite: '#FFFFFF',
  eyeShine: '#FFFFFF',
  ears: '#C4956A',       // slightly darker ears
  tail: '#B8895A',       // tail
  feet: '#C4956A',
  outline: '#8B6D4C',    // warm outline
  magnifier: '#2D9F83',  // sigiro teal
  magnifierGlass: 'rgba(45, 159, 131, 0.15)',
}

function Standing({ size }: { size: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 120 140" fill="none" xmlns="http://www.w3.org/2000/svg">
      {/* Tail */}
      <path d="M75 115 Q 90 105, 92 90 Q 94 80, 88 75" stroke={C.tail} strokeWidth="5" strokeLinecap="round" fill="none" />

      {/* Body */}
      <ellipse cx="60" cy="100" rx="22" ry="30" fill={C.body} />
      {/* Belly */}
      <ellipse cx="60" cy="103" rx="14" ry="22" fill={C.belly} />

      {/* Feet */}
      <ellipse cx="48" cy="128" rx="8" ry="4" fill={C.feet} />
      <ellipse cx="72" cy="128" rx="8" ry="4" fill={C.feet} />

      {/* Arms - relaxed at sides */}
      <path d="M40 90 Q 33 95, 35 105" stroke={C.body} strokeWidth="6" strokeLinecap="round" fill="none" />
      <path d="M80 90 Q 87 95, 85 105" stroke={C.body} strokeWidth="6" strokeLinecap="round" fill="none" />

      {/* Head */}
      <ellipse cx="60" cy="62" rx="20" ry="22" fill={C.face} />

      {/* Ears */}
      <ellipse cx="43" cy="48" rx="6" ry="8" fill={C.ears} />
      <ellipse cx="43" cy="48" rx="3.5" ry="5" fill={C.belly} />
      <ellipse cx="77" cy="48" rx="6" ry="8" fill={C.ears} />
      <ellipse cx="77" cy="48" rx="3.5" ry="5" fill={C.belly} />

      {/* Dark mask around eyes */}
      <ellipse cx="60" cy="58" rx="16" ry="10" fill={C.body} opacity="0.5" />

      {/* Eyes */}
      <ellipse cx="52" cy="57" rx="5" ry="5.5" fill={C.eyeWhite} />
      <ellipse cx="68" cy="57" rx="5" ry="5.5" fill={C.eyeWhite} />
      <ellipse cx="53" cy="57" rx="3.2" ry="3.5" fill={C.eyes} />
      <ellipse cx="69" cy="57" rx="3.2" ry="3.5" fill={C.eyes} />
      {/* Eye shine */}
      <circle cx="54.5" cy="55.5" r="1.2" fill={C.eyeShine} />
      <circle cx="70.5" cy="55.5" r="1.2" fill={C.eyeShine} />

      {/* Nose */}
      <ellipse cx="60" cy="67" rx="3.5" ry="2.5" fill={C.nose} />

      {/* Mouth */}
      <path d="M57 70 Q 60 73, 63 70" stroke={C.nose} strokeWidth="1" fill="none" strokeLinecap="round" />
    </svg>
  )
}

function Magnifying({ size }: { size: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 140 140" fill="none" xmlns="http://www.w3.org/2000/svg">
      {/* Tail */}
      <path d="M85 115 Q 100 105, 102 90 Q 104 80, 98 75" stroke={C.tail} strokeWidth="5" strokeLinecap="round" fill="none" />

      {/* Body */}
      <ellipse cx="65" cy="100" rx="22" ry="30" fill={C.body} />
      <ellipse cx="65" cy="103" rx="14" ry="22" fill={C.belly} />

      {/* Feet */}
      <ellipse cx="53" cy="128" rx="8" ry="4" fill={C.feet} />
      <ellipse cx="77" cy="128" rx="8" ry="4" fill={C.feet} />

      {/* Left arm - holding magnifier handle */}
      <path d="M45 88 Q 30 75, 25 60" stroke={C.body} strokeWidth="6" strokeLinecap="round" fill="none" />

      {/* Right arm - relaxed */}
      <path d="M85 90 Q 92 95, 90 105" stroke={C.body} strokeWidth="6" strokeLinecap="round" fill="none" />

      {/* Magnifying glass */}
      <line x1="25" y1="60" x2="15" y2="42" stroke={C.magnifier} strokeWidth="4" strokeLinecap="round" />
      <circle cx="10" cy="32" r="14" fill={C.magnifierGlass} stroke={C.magnifier} strokeWidth="3" />
      {/* Lens shine */}
      <path d="M4 28 Q 7 24, 12 26" stroke="rgba(255,255,255,0.5)" strokeWidth="1.5" strokeLinecap="round" fill="none" />

      {/* Head - slightly tilted */}
      <ellipse cx="65" cy="62" rx="20" ry="22" fill={C.face} transform="rotate(-5 65 62)" />

      {/* Ears */}
      <ellipse cx="48" cy="46" rx="6" ry="8" fill={C.ears} />
      <ellipse cx="48" cy="46" rx="3.5" ry="5" fill={C.belly} />
      <ellipse cx="82" cy="48" rx="6" ry="8" fill={C.ears} />
      <ellipse cx="82" cy="48" rx="3.5" ry="5" fill={C.belly} />

      {/* Dark mask */}
      <ellipse cx="65" cy="58" rx="16" ry="10" fill={C.body} opacity="0.5" />

      {/* Eyes - looking left toward magnifier */}
      <ellipse cx="57" cy="57" rx="5" ry="5.5" fill={C.eyeWhite} />
      <ellipse cx="73" cy="57" rx="5" ry="5.5" fill={C.eyeWhite} />
      <ellipse cx="55.5" cy="57" rx="3.2" ry="3.5" fill={C.eyes} />
      <ellipse cx="71.5" cy="57" rx="3.2" ry="3.5" fill={C.eyes} />
      <circle cx="57" cy="55.5" r="1.2" fill={C.eyeShine} />
      <circle cx="73" cy="55.5" r="1.2" fill={C.eyeShine} />

      {/* Nose */}
      <ellipse cx="65" cy="67" rx="3.5" ry="2.5" fill={C.nose} />
      <path d="M62 70 Q 65 72, 68 70" stroke={C.nose} strokeWidth="1" fill="none" strokeLinecap="round" />
    </svg>
  )
}

function Peeking({ size }: { size: number }) {
  // Only shows head and paws peeking up from bottom edge
  return (
    <svg width={size} height={size * 0.6} viewBox="0 0 120 72" fill="none" xmlns="http://www.w3.org/2000/svg">
      {/* Paws on edge */}
      <rect x="38" y="60" width="10" height="12" rx="4" fill={C.body} />
      <rect x="72" y="60" width="10" height="12" rx="4" fill={C.body} />

      {/* Head */}
      <ellipse cx="60" cy="38" rx="22" ry="24" fill={C.face} />

      {/* Ears */}
      <ellipse cx="41" cy="20" rx="7" ry="9" fill={C.ears} />
      <ellipse cx="41" cy="20" rx="4" ry="6" fill={C.belly} />
      <ellipse cx="79" cy="20" rx="7" ry="9" fill={C.ears} />
      <ellipse cx="79" cy="20" rx="4" ry="6" fill={C.belly} />

      {/* Dark mask */}
      <ellipse cx="60" cy="34" rx="17" ry="11" fill={C.body} opacity="0.5" />

      {/* Eyes - wide, curious */}
      <ellipse cx="51" cy="33" rx="6" ry="6.5" fill={C.eyeWhite} />
      <ellipse cx="69" cy="33" rx="6" ry="6.5" fill={C.eyeWhite} />
      <ellipse cx="52" cy="33" rx="4" ry="4.5" fill={C.eyes} />
      <ellipse cx="70" cy="33" rx="4" ry="4.5" fill={C.eyes} />
      <circle cx="53.5" cy="31" r="1.5" fill={C.eyeShine} />
      <circle cx="71.5" cy="31" r="1.5" fill={C.eyeShine} />

      {/* Nose */}
      <ellipse cx="60" cy="44" rx="4" ry="3" fill={C.nose} />
      <path d="M56 47 Q 60 50, 64 47" stroke={C.nose} strokeWidth="1.2" fill="none" strokeLinecap="round" />
    </svg>
  )
}

function Looking({ size }: { size: number }) {
  // Same as standing but eyes looking to the side, head slightly turned
  return (
    <svg width={size} height={size} viewBox="0 0 120 140" fill="none" xmlns="http://www.w3.org/2000/svg">
      {/* Tail */}
      <path d="M75 115 Q 90 105, 92 90 Q 94 80, 88 75" stroke={C.tail} strokeWidth="5" strokeLinecap="round" fill="none" />

      {/* Body */}
      <ellipse cx="60" cy="100" rx="22" ry="30" fill={C.body} />
      <ellipse cx="60" cy="103" rx="14" ry="22" fill={C.belly} />

      {/* Feet */}
      <ellipse cx="48" cy="128" rx="8" ry="4" fill={C.feet} />
      <ellipse cx="72" cy="128" rx="8" ry="4" fill={C.feet} />

      {/* Arms - one hand shielding eyes */}
      <path d="M80 90 Q 87 95, 85 105" stroke={C.body} strokeWidth="6" strokeLinecap="round" fill="none" />
      <path d="M42 88 Q 35 78, 42 52" stroke={C.body} strokeWidth="6" strokeLinecap="round" fill="none" />
      {/* Hand near forehead */}
      <ellipse cx="42" cy="50" rx="5" ry="4" fill={C.body} />

      {/* Head */}
      <ellipse cx="60" cy="62" rx="20" ry="22" fill={C.face} />

      {/* Ears */}
      <ellipse cx="43" cy="48" rx="6" ry="8" fill={C.ears} />
      <ellipse cx="43" cy="48" rx="3.5" ry="5" fill={C.belly} />
      <ellipse cx="77" cy="48" rx="6" ry="8" fill={C.ears} />
      <ellipse cx="77" cy="48" rx="3.5" ry="5" fill={C.belly} />

      {/* Dark mask */}
      <ellipse cx="60" cy="58" rx="16" ry="10" fill={C.body} opacity="0.5" />

      {/* Eyes - looking right */}
      <ellipse cx="52" cy="57" rx="5" ry="5.5" fill={C.eyeWhite} />
      <ellipse cx="68" cy="57" rx="5" ry="5.5" fill={C.eyeWhite} />
      <ellipse cx="54.5" cy="57" rx="3.2" ry="3.5" fill={C.eyes} />
      <ellipse cx="70.5" cy="57" rx="3.2" ry="3.5" fill={C.eyes} />
      <circle cx="56" cy="55.5" r="1.2" fill={C.eyeShine} />
      <circle cx="72" cy="55.5" r="1.2" fill={C.eyeShine} />

      {/* Nose */}
      <ellipse cx="60" cy="67" rx="3.5" ry="2.5" fill={C.nose} />
      <path d="M57 70 Q 60 72, 63 70" stroke={C.nose} strokeWidth="1" fill="none" strokeLinecap="round" />
    </svg>
  )
}

export function Meerkat({ pose = 'standing', size = 120, className, style }: MeerkatProps) {
  const props = { size }
  return (
    <span className={className} style={{ display: 'inline-flex', ...style }}>
      {pose === 'standing' && <Standing {...props} />}
      {pose === 'looking' && <Looking {...props} />}
      {pose === 'peeking' && <Peeking {...props} />}
      {pose === 'magnifying' && <Magnifying {...props} />}
    </span>
  )
}

/** Small meerkat head for nav/logo use */
export function MeerkatHead({ size = 28, className }: { size?: number; className?: string }) {
  return (
    <svg className={className} width={size} height={size} viewBox="0 0 40 40" fill="none" xmlns="http://www.w3.org/2000/svg">
      {/* Head */}
      <ellipse cx="20" cy="22" rx="14" ry="15" fill={C.face} />

      {/* Ears */}
      <ellipse cx="8" cy="12" rx="4.5" ry="6" fill={C.ears} />
      <ellipse cx="8" cy="12" rx="2.5" ry="3.5" fill={C.belly} />
      <ellipse cx="32" cy="12" rx="4.5" ry="6" fill={C.ears} />
      <ellipse cx="32" cy="12" rx="2.5" ry="3.5" fill={C.belly} />

      {/* Dark mask */}
      <ellipse cx="20" cy="20" rx="11" ry="7" fill={C.body} opacity="0.5" />

      {/* Eyes */}
      <ellipse cx="14.5" cy="19.5" rx="3.5" ry="4" fill={C.eyeWhite} />
      <ellipse cx="25.5" cy="19.5" rx="3.5" ry="4" fill={C.eyeWhite} />
      <ellipse cx="15.5" cy="19.5" rx="2.2" ry="2.8" fill={C.eyes} />
      <ellipse cx="26.5" cy="19.5" rx="2.2" ry="2.8" fill={C.eyes} />
      <circle cx="16.5" cy="18" r="1" fill={C.eyeShine} />
      <circle cx="27.5" cy="18" r="1" fill={C.eyeShine} />

      {/* Nose */}
      <ellipse cx="20" cy="27" rx="2.5" ry="2" fill={C.nose} />
      <path d="M18 29 Q 20 31, 22 29" stroke={C.nose} strokeWidth="0.8" fill="none" strokeLinecap="round" />
    </svg>
  )
}
