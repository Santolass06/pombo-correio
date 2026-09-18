declare module "qrcode-svg" {
  interface OpcoesQRCode {
    content: string;
    padding?: number;
    width?: number;
    height?: number;
    color?: string;
    background?: string;
    ecl?: "L" | "M" | "Q" | "H";
    join?: boolean;
    xmlDeclaration?: boolean;
  }

  export default class QRCode {
    constructor(opcoes: OpcoesQRCode | string);
    svg(): string;
  }
}
