// SPDX-FileCopyrightText: 2026 Nebula Mail contributors
// SPDX-License-Identifier: AGPL-3.0-only

// Dev-only catalog of the Helix design system: every
// component and state, light and dark. Replaces Storybook/Ladle for now.
// Production builds answer 404 and drop the component.

import {
  AlertDialog,
  AppIcon,
  Avatar,
  Badge,
  Button,
  Card,
  Checkbox,
  ComposeButton,
  ContextMenu,
  Dialog,
  EmptyState,
  Field,
  NebulaLine,
  IconButton,
  IllustrationInboxZero,
  IllustrationNoResults,
  IllustrationPrivacy,
  Input,
  Kbd,
  Mark,
  Menu,
  MenuGroup,
  MenuItem,
  MenuSeparator,
  NavItem,
  Panel,
  Popover,
  PrivacyShield,
  ProgressBar,
  ProgressRing,
  ScrollArea,
  SearchField,
  SegmentedControl,
  Select,
  Separator,
  ShieldGlyph,
  Shortcut,
  Spark,
  Spinner,
  Switch,
  Tab,
  TabList,
  TabPanel,
  Tabs,
  Tag,
  TextLink,
  ThemeScope,
  ToastProvider,
  UIProvider,
  Tooltip,
  Wordmark,
  useToast,
} from "@nebulamail/ui";
import type { ThemeMode } from "@nebulamail/ui";
import { colors, font, space } from "@nebulamail/ui/tokens.stylex";
import * as stylex from "@stylexjs/stylex";
import { createFileRoute, notFound, useRouter } from "@tanstack/react-router";
import {
  AlertTriangle,
  Archive,
  Bell,
  CheckCircle2,
  Clock,
  Crown,
  FileText,
  FileEdit,
  Forward,
  Image,
  Inbox,
  Mail,
  MailOpen,
  MoreHorizontal,
  Newspaper,
  Plus,
  Reply,
  Send,
  Settings,
  ShieldAlert,
  Star,
  Tag as TagIcon,
  Trash2,
  Zap,
} from "lucide-react";
import { useState } from "react";
import type { ReactNode } from "react";

import fontsCss from "@nebulamail/ui/fonts.css?url";

const styles = stylex.create({
  cards: { display: "flex", flexDirection: "column", rowGap: space.lg },
  column: {
    backgroundColor: colors.canvas,
    color: colors.fg,
    minWidth: 0,
    paddingBlock: space.xl,
    paddingInline: space.xl,
  },
  columns: {
    display: "grid",
    gridTemplateColumns: "repeat(2, minmax(0, 1fr))",
  },
  columnsSingle: { gridTemplateColumns: "minmax(0, 1fr)" },
  darkTile: {
    alignItems: "center",
    backgroundColor: "#10111D",
    borderRadius: "12px",
    columnGap: space.md,
    display: "flex",
    paddingBlock: space.md,
    paddingInline: space.lg,
  },
  emptyGrid: {
    display: "grid",
    gap: space.sm,
    gridTemplateColumns: "repeat(auto-fit, minmax(220px, 1fr))",
  },
  header: {
    alignItems: "center",
    backgroundColor: colors.glass,
    backdropFilter: "blur(14px) saturate(1.4)",
    borderBottomColor: colors.line,
    borderBottomStyle: "solid",
    borderBottomWidth: "1px",
    columnGap: space.lg,
    display: "flex",
    paddingBlock: space.s10,
    paddingInline: space.xl,
    position: "sticky",
    top: 0,
    zIndex: 20,
  },
  heading: {
    color: colors.fg3,
    fontSize: font.sizeMd,
    fontWeight: font.weightSemibold,
    letterSpacing: "0.01em",
    marginBlock: "0 14px",
    marginInline: 0,
  },
  label: {
    color: colors.fg3,
    flexShrink: 0,
    fontSize: font.sizeSm,
    width: "84px",
  },
  list: { display: "flex", flexDirection: "column", rowGap: "1px" },
  muted: { color: colors.fg3, fontSize: font.sizeSm, margin: 0 },
  nav: {
    display: "flex",
    flexDirection: "column",
    rowGap: "1px",
    width: "240px",
  },
  panelBody: { height: "200px" },
  panelDemo: { height: "320px" },
  row: {
    alignItems: "center",
    columnGap: space.s10,
    display: "flex",
    flexWrap: "wrap",
    marginBottom: space.md,
    rowGap: space.s10,
  },
  rowTop: { alignItems: "flex-start" },
  spacer: { flexGrow: 1 },
  swatch: (color: string) => ({ backgroundColor: color }),
  swatchBox: {
    borderColor: colors.line,
    borderRadius: "10px",
    borderStyle: "solid",
    borderWidth: "1px",
    height: "40px",
  },
  swatches: {
    display: "grid",
    gap: space.sm,
    gridTemplateColumns: "repeat(auto-fill, minmax(84px, 1fr))",
  },
  swatchName: { color: colors.fg2, fontSize: font.sizeXs, marginTop: "4px" },
  title: {
    fontSize: font.sizeXl,
    fontWeight: font.weightBold,
    letterSpacing: "-0.02em",
    margin: 0,
  },
  twoFields: {
    display: "grid",
    gap: space.md,
    gridTemplateColumns: "repeat(auto-fit, minmax(200px, 1fr))",
    marginBottom: space.md,
  },
  fakeRow: {
    borderRadius: "12px",
    color: colors.fg2,
    fontSize: font.sizeMd,
    paddingBlock: space.s10,
    paddingInline: space.md,
  },
  width: { width: "232px" },
});

const Section = ({
  title,
  children,
}: {
  readonly title: string;
  readonly children: ReactNode;
}) => (
  <Card>
    <h2 {...stylex.props(styles.heading)}>{title}</h2>
    {children}
  </Card>
);

const Row = ({
  label,
  children,
  top = false,
}: {
  readonly label?: string;
  readonly children: ReactNode;
  readonly top?: boolean;
}) => (
  <div {...stylex.props(styles.row, top ? styles.rowTop : null)}>
    {label === undefined ? null : (
      <span {...stylex.props(styles.label)}>{label}</span>
    )}
    {children}
  </div>
);

const SWATCHES = [
  ["canvas", colors.canvas],
  ["bg", colors.bg],
  ["side", colors.side],
  ["raised", colors.raised],
  ["hover", colors.hover],
  ["line", colors.line],
  ["line2", colors.line2],
  ["fg", colors.fg],
  ["fg2", colors.fg2],
  ["fg3", colors.fg3],
  ["acc", colors.acc],
  ["accHi", colors.accHi],
  ["accText", colors.accText],
  ["accSoft", colors.accSoft],
  ["sel", colors.sel],
  ["star", colors.star],
  ["starSoft", colors.starSoft],
  ["ink", colors.ink],
  ["ok", colors.ok],
  ["warn", colors.warn],
  ["danger", colors.danger],
  ["info", colors.info],
  ["n1", colors.n1],
  ["n2", colors.n2],
  ["n3", colors.n3],
  ["core", colors.core],
] as const;

const Tokens = () => (
  <Section title="Colores (tokens)">
    <div {...stylex.props(styles.swatches)}>
      {SWATCHES.map(([name, value]) => (
        <div key={name}>
          <div {...stylex.props(styles.swatchBox, styles.swatch(value))} />
          <div {...stylex.props(styles.swatchName)}>{name}</div>
        </div>
      ))}
    </div>
  </Section>
);

const Brand = () => (
  <Section title="Marca">
    <Row label="Símbolo">
      <Mark size={16} />
      <Mark size={26} />
      <Mark size={52} />
      <Mark size={52} tone="mono" />
      <Mark core="none" size={40} />
    </Row>
    <Row label="Icono">
      <AppIcon size={24} />
      <AppIcon size={48} />
      <AppIcon size={72} />
      <AppIcon light size={72} />
    </Row>
    <Row label="Logotipo">
      <Mark size={26} />
      <Wordmark />
      <Wordmark size="lg" />
      <span {...stylex.props(styles.darkTile)}>
        <Mark size={30} tone="onDark" />
      </span>
    </Row>
    <Row label="Motivo">
      <Spark size={20} />
      <Spark double size={20} />
      <span {...stylex.props(styles.width)}>
        <NebulaLine />
      </span>
      <ShieldGlyph size={24} />
    </Row>
  </Section>
);

const Buttons = () => (
  <Section title="Botones">
    <Row label="Primario">
      <Button icon={Send} variant="primary">
        Enviar
      </Button>
      <Button shortcut={["⌘", "↵"]} variant="primary">
        Enviar
      </Button>
      <Button loading variant="primary">
        Enviando
      </Button>
      <Button disabled variant="primary">
        Desactivado
      </Button>
    </Row>
    <Row label="Secundario">
      <Button icon={Reply}>Responder</Button>
      <Button icon={Image} size="sm">
        Mostrar imágenes
      </Button>
      <Button disabled>Desactivado</Button>
    </Row>
    <Row label="Fantasma">
      <Button icon={Archive} shortcut={["E"]} variant="ghost">
        Hecho
      </Button>
      <Button pressed variant="ghost">
        Activo
      </Button>
      <Button variant="ink">Tinta</Button>
    </Row>
    <Row label="Peligro">
      <Button icon={Trash2} variant="danger">
        Eliminar para siempre
      </Button>
      <Button variant="dangerSoft">Vaciar spam</Button>
    </Row>
    <Row label="Tamaños">
      <Button size="sm" variant="primary">
        Pequeño
      </Button>
      <Button variant="primary">Normal</Button>
      <Button size="lg" variant="primary">
        Grande
      </Button>
    </Row>
    <Row label="Icono">
      <IconButton icon={Settings} label="Ajustes" />
      <IconButton icon={Archive} label="Hecho" shortcut={["E"]} size="sm" />
      <IconButton icon={Star} label="Destacar" pressed size="sm" />
      <IconButton icon={Plus} label="Añadir" size="xs" />
      <IconButton icon={Bell} label="Avisos" variant="secondary" />
      <IconButton disabled icon={Trash2} label="Eliminar" />
    </Row>
    <Row label="Redactar">
      <span {...stylex.props(styles.width)}>
        <ComposeButton shortcut={["C"]}>Nuevo mensaje</ComposeButton>
      </span>
      <ComposeButton collapsed shortcut={["C"]}>
        Nuevo mensaje
      </ComposeButton>
    </Row>
  </Section>
);

const Badges = () => (
  <Section title="Teclas, insignias y etiquetas">
    <Row label="Teclas">
      <Kbd>E</Kbd>
      <Shortcut keys={["⌘", "K"]} />
      <Shortcut keys={["Ctrl", "K"]} />
      <Shortcut keys={["⇧", "O"]} />
      <Shortcut keys={["G", "I"]} />
    </Row>
    <Row label="Insignias">
      <Badge mark tone="brand">
        Oficial
      </Badge>
      <Badge icon={CheckCircle2} tone="ok">
        Verificado
      </Badge>
      <Badge tone="star">VIP</Badge>
      <Badge icon={AlertTriangle} tone="warn">
        Suplantación posible
      </Badge>
      <Badge icon={ShieldAlert} tone="danger">
        Spam
      </Badge>
      <Badge tone="info">Nuevo</Badge>
      <Badge>Borrador</Badge>
    </Row>
    <Row label="Etiquetas">
      <Tag hue={175}>Casa</Tag>
      <Tag hue={345}>Familia</Tag>
      <Tag hue={215}>Trabajo</Tag>
      <Tag hue={38}>Facturas</Tag>
      <Tag hue={140}>Viajes</Tag>
      <Tag hue={15}>Club de lectura</Tag>
    </Row>
  </Section>
);

// A tiny local image (CSP img-src allows data:), standing in for a proxied logo.
const SAMPLE_IMAGE = `data:image/svg+xml,${encodeURIComponent(
  '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 40 40"><rect width="40" height="40" fill="#2353C9"/><circle cx="20" cy="20" r="9" fill="#fff"/></svg>'
)}`;

const Avatars = () => (
  <Section title="Avatares">
    <Row>
      <Avatar name="Nebula Mail" official size={40} />
      <Avatar name="Marta Iglesias" size={40} />
      <Avatar name="Carlos Vidal" size={40} />
      <Avatar colorKey="bancohorizonte.es" name="Banco Horizonte" size={40} />
      <Avatar name="Elena Ruiz" size={40} />
      <Avatar name="La Brújula Semanal" size={40} />
      <Avatar
        decorative={false}
        name="Logo por el proxy"
        size={40}
        src={SAMPLE_IMAGE}
      />
    </Row>
    <Row>
      <Avatar name="Marta Iglesias" size={34} unread />
      <Avatar name="Laura Gómez" size={24} />
      <Avatar name="Óscar Núñez" size={32} />
      <p {...stylex.props(styles.muted)}>
        Iniciales sobre un tono derivado del nombre o del dominio; punto de no
        leído en índigo.
      </p>
    </Row>
  </Section>
);

const Fields = () => {
  const [mac, setMac] = useState(true);
  return (
    <Section title="Campos y búsqueda">
      <Row>
        <span {...stylex.props(styles.spacer)}>
          <SearchField
            hint={<Shortcut keys={mac ? ["⌘", "K"] : ["Ctrl", "K"]} />}
            label="Buscar mensajes"
            placeholder="Buscar mensajes"
          />
        </span>
        <Switch checked={mac} label="Mac" onCheckedChange={setMac} />
      </Row>
      <div {...stylex.props(styles.twoFields)}>
        <Field description="Reposo" label="Asunto">
          <Input placeholder="Escribe un asunto" />
        </Field>
        <Field description="Con icono" label="Para">
          <Input defaultValue="marta@iglesiasreformas.es" icon={Mail} />
        </Field>
        <Field
          error="Falta el dominio (p. ej. @iglesiasreformas.es)"
          label="Correo de respuesta"
        >
          <Input defaultValue="marta@iglesias" />
        </Field>
        <Field description="Lo bloquea el administrador" disabled label="Firma">
          <Input defaultValue="Desactivado" />
        </Field>
      </div>
    </Section>
  );
};

const FILTERS = [
  { label: "Todos", value: "all" },
  { label: "No leídos", value: "unread" },
  { label: "Leídos", value: "read" },
  { label: "Adjuntos", value: "attachments" },
] as const;

const Selection = () => (
  <Section title="Casillas, interruptores y filtros">
    <Row>
      <Checkbox label="Sin marcar" />
      <Checkbox defaultChecked label="Marcada" />
      <Checkbox indeterminate label="Mixta" />
      <Checkbox disabled label="Desactivada" />
    </Row>
    <Row>
      <Switch defaultChecked label="Quitar rastreadores de imágenes" />
    </Row>
    <Row>
      <Switch label="Mostrar imágenes de remitentes de confianza" />
      <Switch disabled label="Desactivado" />
    </Row>
    <Row>
      <SegmentedControl
        defaultValue="all"
        label="Filtro rápido"
        options={FILTERS}
      />
    </Row>
  </Section>
);

const LEVELS = [
  { label: "Puede ver", value: "read" },
  { label: "Puede ver y editar", value: "write" },
  { label: "Puede editar y compartir", value: "manage" },
  { disabled: true, label: "Personalizado (otra app)", value: "custom" },
] as const;

const ZONE_GROUPS = [
  { label: "Ninguna", value: "" },
  {
    label: "Europa",
    options: [
      { label: "Lisboa (GMT)", value: "Europe/Lisbon" },
      { label: "Madrid (GMT+1)", value: "Europe/Madrid" },
      { label: "París (GMT+1)", value: "Europe/Paris" },
    ],
  },
  {
    label: "América",
    options: [
      { label: "Bogotá (GMT-5)", value: "America/Bogota" },
      { label: "Ciudad de México (GMT-6)", value: "America/Mexico_City" },
      { label: "Nueva York (GMT-5)", value: "America/New_York" },
    ],
  },
] as const;

/** Every zone the browser knows: to check that a long list stays smooth. */
const allZones = (): { label: string; value: string }[] =>
  Intl.supportedValuesOf("timeZone").map((zone) => ({
    label: zone.replaceAll("_", " "),
    value: zone,
  }));

const Dropdowns = () => {
  const [level, setLevel] = useState<string>("write");
  const [zone, setZone] = useState<string>("Europe/Madrid");
  const [longZone, setLongZone] = useState<string>("Europe/Madrid");
  const [every, setEvery] = useState<string>("weekly");
  return (
    <Section title="Desplegables (Select)">
      <Field label="Permiso">
        <Select
          label="Permiso"
          onValueChange={setLevel}
          options={LEVELS}
          value={level}
        />
      </Field>
      <Row>
        <span>Repetir cada</span>
        <Select
          label="Frecuencia"
          onValueChange={setEvery}
          options={[
            { label: "día", value: "daily" },
            { label: "semana", value: "weekly" },
            { label: "mes", value: "monthly" },
            { label: "año", value: "yearly" },
          ]}
          value={every}
          width="fit"
        />
      </Row>
      <Field label="Zona horaria (con grupos)">
        <Select
          label="Zona horaria (con grupos)"
          onValueChange={setZone}
          options={ZONE_GROUPS}
          value={zone}
        />
      </Field>
      <Field
        description="Todas las del navegador; escribe para saltar a una."
        label="Zona horaria (lista larga)"
      >
        <Select
          label="Zona horaria (lista larga)"
          onValueChange={setLongZone}
          options={allZones()}
          value={longZone}
        />
      </Field>
      <Row>
        <Select
          disabled
          label="Desactivado"
          onValueChange={setLevel}
          options={LEVELS}
          value={level}
          width="fit"
        />
        <Select
          invalid
          label="No válido"
          onValueChange={setLevel}
          options={LEVELS}
          value={level}
          width="fit"
        />
      </Row>
    </Section>
  );
};

const SplitTabs = () => (
  <Section title="Pestañas (bandeja dividida)">
    <Tabs defaultValue="important">
      <TabList label="Bandeja dividida">
        <Tab count={7} icon={Zap} value="important">
          Importante
        </Tab>
        <Tab count={2} icon={Crown} value="vip">
          VIP
        </Tab>
        <Tab count={12} icon={Newspaper} value="news">
          Noticias
        </Tab>
        <Tab count={5} icon={Bell} value="notices">
          Avisos
        </Tab>
        <Tab count={1} icon={MailOpen} value="other">
          Otros
        </Tab>
        <IconButton icon={Plus} label="Nueva división" size="sm" />
      </TabList>
      <TabPanel value="important">
        <p {...stylex.props(styles.muted)}>
          La hebra doble subraya la pestaña activa.
        </p>
      </TabPanel>
      <TabPanel value="vip">
        <p {...stylex.props(styles.muted)}>Dos conversaciones VIP.</p>
      </TabPanel>
      <TabPanel value="news">
        <p {...stylex.props(styles.muted)}>Boletines.</p>
      </TabPanel>
      <TabPanel value="notices">
        <p {...stylex.props(styles.muted)}>Avisos.</p>
      </TabPanel>
      <TabPanel value="other">
        <p {...stylex.props(styles.muted)}>Lo demás.</p>
      </TabPanel>
    </Tabs>
  </Section>
);

const MESSAGE_MENU = (
  <>
    <MenuItem icon={Reply} shortcut={["R"]}>
      Responder
    </MenuItem>
    <MenuItem icon={Forward} shortcut={["F"]}>
      Reenviar
    </MenuItem>
    <MenuItem icon={Clock} shortcut={["H"]}>
      Posponer…
    </MenuItem>
    <MenuItem icon={FileText} shortcut={["⇧", "O"]}>
      Ver original
    </MenuItem>
    <MenuGroup label="Etiquetar">
      <MenuItem icon={TagIcon} keepOpen>
        Trabajo
      </MenuItem>
      <MenuItem disabled icon={TagIcon}>
        Casa (desactivado)
      </MenuItem>
    </MenuGroup>
    <MenuSeparator />
    <MenuItem danger icon={Trash2} shortcut={["#"]}>
      Eliminar
    </MenuItem>
  </>
);

const Overlays = () => {
  const toast = useToast();
  return (
    <Section title="Menús, ventanas y avisos">
      <Row label="Menú">
        <Menu trigger={<Button iconEnd={MoreHorizontal}>Más acciones</Button>}>
          {MESSAGE_MENU}
        </Menu>
        <Menu
          align="end"
          trigger={
            <IconButton icon={MoreHorizontal} label="Más" tooltip={false} />
          }
        >
          {MESSAGE_MENU}
        </Menu>
        <ContextMenu menu={MESSAGE_MENU}>
          <span {...stylex.props(styles.muted)}>
            Clic derecho aquí para el menú contextual
          </span>
        </ContextMenu>
      </Row>
      <Row label="Tooltip">
        <Tooltip content="Hecho" shortcut={["E"]}>
          <Button icon={Archive} variant="ghost">
            Pasa por encima
          </Button>
        </Tooltip>
      </Row>
      <Row label="Ventanas">
        <Dialog.Root>
          <Dialog.Trigger
            render={<Button icon={Clock}>Abrir diálogo</Button>}
          />
          <Dialog.Content closeLabel="Cerrar">
            <Dialog.Title>Posponer conversación</Dialog.Title>
            <Dialog.Description>
              Elige cuándo quieres volver a ver esta conversación.
            </Dialog.Description>
            <Dialog.Body>
              <p>Mañana a las 8:00 · El lunes a las 8:00 · Elegir fecha…</p>
            </Dialog.Body>
            <Dialog.Footer>
              <Button>Cancelar</Button>
              <Button variant="primary">Posponer</Button>
            </Dialog.Footer>
          </Dialog.Content>
        </Dialog.Root>
        <AlertDialog
          cancelLabel="Cancelar"
          confirmLabel="Eliminar para siempre"
          description="Se eliminarán 12 conversaciones de la papelera. No se puede deshacer."
          onConfirm={() => {
            toast.show({ title: "Papelera vaciada", tone: "success" });
          }}
          title="¿Vaciar la papelera?"
          tone="danger"
          trigger={<Button variant="dangerSoft">Vaciar papelera</Button>}
        />
        <Popover.Root>
          <Popover.Trigger
            render={<Button icon={ShieldAlert}>Ver informe</Button>}
          />
          <Popover.Content variant="accent">
            <Popover.Header icon={<ShieldGlyph size={22} />}>
              <Popover.Title>Informe de privacidad</Popover.Title>
              <Popover.Description>
                Esto es lo que hemos hecho por ti en este correo
              </Popover.Description>
            </Popover.Header>
            <Popover.Section>
              <b>3 rastreadores bloqueados</b>
              <p {...stylex.props(styles.muted)}>
                El remitente no sabrá cuándo abres el correo.
              </p>
            </Popover.Section>
            <Popover.Section>
              <b>Imágenes protegidas</b>
              <p {...stylex.props(styles.muted)}>
                Se cargan por nuestro proxy y tu IP queda oculta.
              </p>
            </Popover.Section>
          </Popover.Content>
        </Popover.Root>
      </Row>
      <Row label="Avisos">
        <Button
          icon={Send}
          onClick={() => {
            toast.show({
              action: {
                label: "Deshacer",
                onClick: () => {
                  toast.show({ title: "Envío cancelado" });
                },
              },
              timeout: 10_000,
              title: "Enviado a Marta Iglesias",
              tone: "success",
            });
          }}
          variant="primary"
        >
          Enviar (con Deshacer)
        </Button>
        <Button
          onClick={() => {
            toast.show({
              description: "Tus borradores están a salvo en este dispositivo.",
              priority: "high",
              title: "No podemos conectar con tu servidor",
              tone: "danger",
            });
          }}
        >
          Error
        </Button>
        <Button
          onClick={() => {
            toast.show({ title: "2 conversaciones archivadas" });
          }}
          variant="ghost"
        >
          Neutro
        </Button>
      </Row>
    </Section>
  );
};

const Navigation = () => (
  <Section title="Navegación y progreso">
    <Row top>
      <nav aria-label="Carpetas" {...stylex.props(styles.nav)}>
        <NavItem active count={7} href="#recibidos" icon={Inbox}>
          Recibidos
        </NavItem>
        <NavItem count={2} href="#borradores" icon={FileEdit} mutedCount>
          Borradores
        </NavItem>
        <NavItem href="#enviados" icon={Send}>
          Enviados
        </NavItem>
        <NavItem href="#destacados" icon={Star}>
          Destacados
        </NavItem>
        <NavItem count={1} href="#pospuestos" icon={Clock} mutedCount>
          Pospuestos
        </NavItem>
        <NavItem href="#familia" hue={345}>
          Familia
        </NavItem>
        <NavItem href="#trabajo" hue={215}>
          Trabajo
        </NavItem>
      </nav>
      <div {...stylex.props(styles.spacer)}>
        <Row>
          <ProgressRing label="Configuración: 2 de 4 pasos" max={4} value={2} />
          <span>
            <b>Termina de configurar</b>
            <p {...stylex.props(styles.muted)}>2 de 4 pasos</p>
          </span>
        </Row>
        <ProgressBar
          end={<TextLink href="#gestionar">Gestionar</TextLink>}
          label="1,2 GB de 15 GB"
          max={15}
          value={1.2}
        />
        <ProgressBar
          aria-label="Subiendo informe.pdf"
          busyText="Subiendo…"
          value={null}
        />
        <Row>
          <Spinner label="Cargando" />
          <Spinner label="Cargando" size="sm" />
          <Button loading size="sm">
            Cargando
          </Button>
        </Row>
      </div>
    </Row>
  </Section>
);

const ROWS = [
  "Marta, yo · Re: Presupuesto reforma cocina",
  "Carlos Vidal · Cena del sábado: ¿a las 21:30?",
  "Banco Horizonte · Tu extracto de octubre ya está disponible",
  "Elena, Pablo, yo · Revisión de diseño — jueves 24",
  "Trenes del Norte · Tu billete Madrid → Santander",
  "Laura Gómez · Libro de noviembre para el club",
  "Gestoría Ramos · Documentación pendiente del IRPF",
  "Pablo Sanz · Fotos del viaje a Asturias",
  "Clínica Dental Sonrisa · Recordatorio: cita el martes 29",
] as const;

const Structure = () => (
  <Section title="Panel, desplazamiento y separadores">
    <Panel label="Lista de mensajes" xstyle={styles.panelDemo}>
      <Row>
        <span />
        <SegmentedControl defaultValue="all" label="Filtro" options={FILTERS} />
        <Separator orientation="vertical" />
        <IconButton icon={Archive} label="Hecho" shortcut={["E"]} size="sm" />
        <IconButton icon={Trash2} label="Eliminar" shortcut={["#"]} size="sm" />
      </Row>
      <Separator />
      <ScrollArea xstyle={styles.panelBody}>
        <div {...stylex.props(styles.list)}>
          {ROWS.map((row) => (
            <div key={row} {...stylex.props(styles.fakeRow)}>
              {row}
            </div>
          ))}
        </div>
      </ScrollArea>
    </Panel>
  </Section>
);

const Empty = () => (
  <Section title="Estados vacíos">
    <div {...stylex.props(styles.emptyGrid)}>
      <EmptyState illustration={<IllustrationInboxZero />} title="Todo al día">
        No queda nada por leer. Disfruta del silencio.
      </EmptyState>
      <EmptyState
        action={<Button size="sm">Buscar en todo</Button>}
        illustration={<IllustrationNoResults />}
        title="Nada con «factura marzo»"
      >
        Prueba con el remitente o busca en todas las carpetas.
      </EmptyState>
      <EmptyState
        illustration={<IllustrationPrivacy />}
        title="214 rastreadores bloqueados"
      >
        Este mes, ningún remitente ha sabido cuándo abrías sus correos.
      </EmptyState>
    </div>
  </Section>
);

const Privacy = () => {
  const [blocked, setBlocked] = useState(2);
  return (
    <Section title="Escudo de privacidad">
      <Row>
        <PrivacyShield label="Protección activa" pulse={blocked} size={40} />
        <span>
          <b>{blocked}</b> rastreadores bloqueados
        </span>
        <Button
          onClick={() => {
            setBlocked((count) => count + 1);
          }}
          size="sm"
        >
          Bloquear otro (pulso)
        </Button>
      </Row>
      <p {...stylex.props(styles.muted)}>
        Un solo pulso de luz cuando cambia el recuento. Nunca en bucle; sin
        animación con «reducir movimiento».
      </p>
    </Section>
  );
};

const Column = ({ mode }: { readonly mode: ThemeMode }) => (
  <ThemeScope mode={mode}>
    <ToastProvider>
      <div {...stylex.props(styles.column)}>
        <div {...stylex.props(styles.cards)}>
          <Brand />
          <Buttons />
          <Badges />
          <Avatars />
          <Fields />
          <Selection />
          <Dropdowns />
          <SplitTabs />
          <Overlays />
          <Navigation />
          <Structure />
          <Privacy />
          <Empty />
          <Tokens />
        </div>
      </div>
    </ToastProvider>
  </ThemeScope>
);

type View = "both" | "light" | "dark";

const VIEWS = [
  { label: "Claro y oscuro", value: "both" },
  { label: "Claro", value: "light" },
  { label: "Oscuro", value: "dark" },
] as const;

const Catalog = () => {
  const [view, setView] = useState<View>("both");
  const nonce = useRouter().options.ssr?.nonce;
  const modes: readonly ThemeMode[] =
    view === "both" ? ["light", "dark"] : [view];
  return (
    // The app root will provide UIProvider; the catalog brings its own so it
    // works before that lands.
    <UIProvider {...(nonce === undefined ? {} : { nonce })}>
      <ThemeScope mode="system">
        <header {...stylex.props(styles.header)}>
          <Mark size={26} />
          <Wordmark />
          <h1 {...stylex.props(styles.title)}>Helix · catálogo</h1>
          <span {...stylex.props(styles.spacer)} />
          <SegmentedControl
            label="Tema"
            onValueChange={setView}
            options={VIEWS}
            value={view}
          />
        </header>
        <main
          {...stylex.props(
            styles.columns,
            modes.length === 1 ? styles.columnsSingle : null
          )}
        >
          {modes.map((mode) => (
            <Column key={mode} mode={mode} />
          ))}
        </main>
      </ThemeScope>
    </UIProvider>
  );
};

export const Route = createFileRoute("/dev/ui")({
  beforeLoad: () => {
    if (!import.meta.env.DEV) {
      throw notFound();
    }
  },
  // In production the constant folds to null and the catalog is dropped.
  component: import.meta.env.DEV ? Catalog : () => null,
  head: () => ({
    links: [{ href: fontsCss, rel: "stylesheet" }],
    meta: [{ title: "Helix · catálogo" }],
  }),
});
