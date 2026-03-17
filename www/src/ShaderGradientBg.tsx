import { ShaderGradientCanvas, ShaderGradient } from '@shadergradient/react'

export default function ShaderGradientBg() {
  return (
    <ShaderGradientCanvas
      style={{ width: '100%', height: '100%' }}
      pixelDensity={1}
      pointerEvents="none"
      lazyLoad
    >
      <ShaderGradient
        type="waterPlane"
        animate="on"
        uSpeed={0.3}
        uStrength={1.5}
        uDensity={1.8}
        uFrequency={3.5}
        uAmplitude={3}
        color1="#C4501A"
        color2="#D4943E"
        color3="#F7F4EF"
        positionX={0}
        positionY={0}
        positionZ={0}
        rotationX={0}
        rotationY={0}
        rotationZ={0}
        reflection={0.1}
        wireframe={false}
        cAzimuthAngle={180}
        cPolarAngle={80}
        cDistance={3.6}
        cameraZoom={1}
        lightType="3d"
        brightness={1.2}
        envPreset="city"
        grain="off"
      />
    </ShaderGradientCanvas>
  )
}
