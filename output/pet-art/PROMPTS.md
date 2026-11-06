# Character pet artwork — pixel edition

Generated with built-in `image_gen` on 2026-11-05, using the three existing cat idle images as style references and each previous character sheet as the edit target. Active source sheets: `output/pet-art/{nailong,doraemon,tihun}-sheet.png`. Active runtime assets: `src-tauri/pets/{nailong,doraemon,tihun}/`. Previous 3D artwork, prompts and previews are archived under `output/pet-art/previous-3d/`.

Each sheet is sliced with `swift scripts/prepare-character-pets.swift output/pet-art/<id>-sheet.png src-tauri/pets/<id>`. The tool crops and cleans the generated sprites, aligns their feet on a 128px grid, makes a one-row breathing pose, and enlarges exactly 4× with nearest-neighbor sampling into 512px APNGs. Both desktop and settings thumbnails use pixelated rendering.

Style references (unaltered): `british-blue/idle.png`, `douhua-cat/idle.png`, `huajuan-cat/idle.png`, from the existing Familiar packs. The generated characters are fan depictions, not official artwork or a grant of rights in the characters. Ti Hun identity reference remains the animation's green monkey form (see previous-3d/PROMPTS.md).

## Shared prompt

Use case: style-transfer.
Asset: desktop pet sprite sheet, 8 expressive full-body poses, one sheet, strict 4 columns × 2 rows of equal square cells, wide 2:1 canvas, TRUE transparent background.
Inputs: images 1–3 are STYLE REFERENCES ONLY (the user's existing pixel-art cats). Image 4 is the EDIT TARGET, a sheet of the requested character in 8 poses.
Primary request: REDRAW image 4 entirely in the beautiful detailed PIXEL ART of the reference cats. The user specifically rejected the smooth 3D-rendered look of image 4. Preserve character identity, palette, iconic features, eight poses and expression order. Match the references' confident near-black stepped pixel outlines, chubby cute proportions, expressive large eyes with small square catchlights, thoughtfully shaped flat-color pixel clusters, rich but restrained 5–7-tone shading ramps, hand-placed pixel highlights and shadow clusters. Fine lovingly crafted 16-bit / 32-bit game sprite aesthetic, approximately 96×96 logical pixels per figure enlarged with NEAREST NEIGHBOR. Fur where appropriate is jagged tuft-shaped pixel clusters like the fluffy reference cats. Every visible edge must be made of crisp square pixel steps, consistent pixel scale across all 8 poses.
NOT a 3D render with a pixelation filter, no smooth vector curves, no airbrush, no realistic textures, no shiny toy material, no dithering noise, no very coarse 16×16 stick figures.
Layout: 4 columns and 2 rows, all 8 figures COMPLETE, each figure strictly within its own cell with at least 8% transparent margin on every side. Same stable scale and baseline. No overlaps. Avoid filling cell edges. No reference cats in the output.
Reading order:
top: idle relaxed; working focused hands together; thinking hand at chin; interacting happy wave.
bottom: celebrating raised arms joyful; alarmed startled hands at cheeks; sleeping sitting curled with eyes closed; watching curious looking up waiting.
Transparent backdrop only, NO checkerboard pattern copied from references, NO color backdrop, NO text, NO labels, NO watermark, NO ground or drop shadow. All color confined to the character silhouette. Keep recognizable expressive details readable at 72px.

## nailong

Character invariant: 奶龙 Nailoong, golden yellow baby dragon, huge chubby pear belly, cream belly patch, big green eyes, stubby limbs, tiny brown claws, small tail and dorsal spines. Keep the lovable silly canonical face.

## doraemon

Character invariant: 哆啦 A 梦 Doraemon, blue spherical robot cat, white face/belly, semicircular belly pocket, red round nose, paired white oval eyes, three whiskers on each cheek, NO ears, red collar and gold bell, white round hands and feet. Preserve canonical identity in true pixel art.

## tihun

Character invariant: 啼魂兽 Ti Hun from 凡人修仙传, little green monkey spirit, jade-green fur, mint tuft and cheek ruff rendered as jagged pixel fur clusters like the reference cats, pale mint cream belly, BLUE-gray face and tiny blue hands/feet, large amber eyes, broad blue muzzle with flat nostrils, rounded green ears, curled tail. Preserve the cute small monkey form, not a gorilla or generic cat.
