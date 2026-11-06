# Character pet artwork

Generated with built-in `image_gen` on 2026-11-05. Source sheets: `output/pet-art/*-sheet.png`. Runtime assets: `src-tauri/pets/{nailong,doraemon,tihun}/`. Mechanical slicing, alpha cleanup, fixed baseline and subtle two-frame breathing: `swift scripts/prepare-character-pets.swift output/pet-art/<id>-sheet.png src-tauri/pets/<id>`.

These are generated fan depictions of the requested characters, not official artwork or a grant of rights in the characters. Animation reference for Ti Hun: https://www.sohu.com/a/769858742_121922792 (reference image only, not bundled runtime art). Nailoong visual identity reference: https://www.nailoong.com/business/overseasBusiness/.

## Shared prompt

Use case: stylized-concept.
Asset type: production transparent desktop-pet state sprite sheet. Generate one wide 2048x1024 PNG, exact 4 columns by 2 rows of equal 512x512 square cells, all on TRUE TRANSPARENT background. NO text, NO labels, NO borders, NO checkerboard, no floor plane, no shadows outside character. Eight full-body depictions of the SAME character with IDENTICAL anatomy, materials, lighting, color and camera, polished high-end 3D animated film / collectible character quality. Rich subtle soft shading, clear silhouette, readable expressive face at 72px, rounded charming proportions. Every character fully fits within its own square with 12 percent clear margin on ALL sides; never touch a cell boundary. Feet baseline at 88 percent of each cell, character horizontally centered. Use stable frontal to slight three-quarter camera. Expressions in strict reading order:
Top row: 1 idle: friendly relaxed standing; 2 working: focused determined expression, hands together busily concentrating, no props; 3 thinking: head slightly tilted, one hand on chin; 4 interacting: smiling warmly, one hand raised to wave.
Bottom row: 5 celebrating: joyful laugh both hands up, keep inside margin; 6 alarmed: startled worried wide eyes, hands to cheeks; 7 sleeping: seated curled sleepy body with closed eyes, no bed or props; 8 watching: attentive curious looking up as if awaiting reply, hands resting on belly.
Avoid flat clip art, pixel art, thick black outlines, low detail, duplicate poses, deformed hands, extra limbs, costumes, extraneous objects, any lettering.

## nailong

Primary request: 奶龙 Nailoong / Nailong, the recognizable canonical chubby golden yellow baby dragon from 小七和奶龙. Huge pear-shaped round plump belly with pale buttery cream belly patch, smooth golden yellow skin, big oval green eyes with white sclera and black pupils, tiny nostrils, wide cute cheeky mouth, very short arms and legs with tiny brown claws, a small chunky tail, small subtle dorsal spines. No large horns, no wings. Preserve the original character's lovable silly expression and distinctive broad head and belly.

## doraemon

Primary request: the canonical 哆啦A梦 Doraemon robot cat. Blue spherical head and squat round blue body, white round face and white belly with semicircular four-dimensional pocket, paired large white oval eyes touching in middle, round bright red nose, three thin whiskers each cheek, no ears, white spherical hands, white broad feet, red collar with small shiny gold bell. Friendly expressive original face. Beautiful smooth satin blue material, white soft surfaces, polished 3D theatrical movie quality. Maintain canonical Doraemon proportions and identity across all eight poses.

## tihun

Primary request: 啼魂兽 Ti Hun, Han Li's SMALL GREEN MONKEY spirit companion in the Chinese 3D animated 凡人修仙传 A Record of a Mortal's Journey to Immortality. NOT a human or gorilla or generic dragon. A tiny round chubby baby monkey with plush emerald / jade green fur, fluffy pale mint swept-up head tuft and cheek ruff, contrasting blue gray / cyan bare face and blue tiny hands and feet, very large glossy amber-brown eyes with dark lids, short broad blue muzzle with prominent flat nostrils, pale cream mint belly, big rounded green ears, little monkey tail curled beside body. Recognizably the adorable animation-version Ti Hun. Beautiful fine individual fur detail, soft ruff, lovable curious intelligent gaze, not scary, no clothing. Preserve green fur and BLUE face.
