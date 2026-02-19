"""Public API surface shim for ``email._header_value_parser``."""

from __future__ import annotations

import re as _re

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_capabilities_has", globals())


class AddrSpec:
    pass


class Address:
    pass


class AddressList:
    pass


class AngleAddr:
    pass


class Atom:
    pass


class Attribute:
    pass


class BareQuotedString:
    pass


class CFWSList:
    pass


class Comment:
    pass


class ContentDisposition:
    pass


class ContentTransferEncoding:
    pass


class ContentType:
    pass


class DisplayName:
    pass


class Domain:
    pass


class DomainLiteral:
    pass


class DotAtom:
    pass


class DotAtomText:
    pass


class EWWhiteSpaceTerminal:
    pass


class EncodedWord:
    pass


class Group:
    pass


class GroupList:
    pass


class Header:
    pass


class HeaderLabel:
    pass


class InvalidMailbox:
    pass


class InvalidMessageID:
    pass


class InvalidParameter:
    pass


class LocalPart:
    pass


class MIMEVersion:
    pass


class Mailbox:
    pass


class MailboxList:
    pass


class MessageID:
    pass


class MimeParameters:
    pass


class MsgID:
    pass


class NameAddr:
    pass


class NoFoldLiteral:
    pass


class ObsLocalPart:
    pass


class ObsRoute:
    pass


class Parameter:
    pass


class ParameterizedHeaderValue:
    pass


class Phrase:
    pass


class QuotedString:
    pass


class Section:
    pass


class Terminal:
    pass


class Token:
    pass


class TokenList:
    pass


class UnstructuredTokenList:
    pass


class Value:
    pass


class ValueTerminal:
    pass


class WhiteSpaceTerminal:
    pass


class WhiteSpaceTokenList:
    pass


class Word:
    pass


class itemgetter:
    pass


def get_addr_spec(*args, **kwargs):
    del args, kwargs
    return None


def get_address(*args, **kwargs):
    del args, kwargs
    return None


def get_address_list(*args, **kwargs):
    del args, kwargs
    return None


def get_angle_addr(*args, **kwargs):
    del args, kwargs
    return None


def get_atext(*args, **kwargs):
    del args, kwargs
    return None


def get_atom(*args, **kwargs):
    del args, kwargs
    return None


def get_attribute(*args, **kwargs):
    del args, kwargs
    return None


def get_attrtext(*args, **kwargs):
    del args, kwargs
    return None


def get_bare_quoted_string(*args, **kwargs):
    del args, kwargs
    return None


def get_cfws(*args, **kwargs):
    del args, kwargs
    return None


def get_comment(*args, **kwargs):
    del args, kwargs
    return None


def get_display_name(*args, **kwargs):
    del args, kwargs
    return None


def get_domain(*args, **kwargs):
    del args, kwargs
    return None


def get_domain_literal(*args, **kwargs):
    del args, kwargs
    return None


def get_dot_atom(*args, **kwargs):
    del args, kwargs
    return None


def get_dot_atom_text(*args, **kwargs):
    del args, kwargs
    return None


def get_dtext(*args, **kwargs):
    del args, kwargs
    return None


def get_encoded_word(*args, **kwargs):
    del args, kwargs
    return None


def get_extended_attribute(*args, **kwargs):
    del args, kwargs
    return None


def get_extended_attrtext(*args, **kwargs):
    del args, kwargs
    return None


def get_fws(*args, **kwargs):
    del args, kwargs
    return None


def get_group(*args, **kwargs):
    del args, kwargs
    return None


def get_group_list(*args, **kwargs):
    del args, kwargs
    return None


def get_invalid_mailbox(*args, **kwargs):
    del args, kwargs
    return None


def get_invalid_parameter(*args, **kwargs):
    del args, kwargs
    return None


def get_local_part(*args, **kwargs):
    del args, kwargs
    return None


def get_mailbox(*args, **kwargs):
    del args, kwargs
    return None


def get_mailbox_list(*args, **kwargs):
    del args, kwargs
    return None


def get_msg_id(*args, **kwargs):
    del args, kwargs
    return None


def get_name_addr(*args, **kwargs):
    del args, kwargs
    return None


def get_no_fold_literal(*args, **kwargs):
    del args, kwargs
    return None


def get_obs_local_part(*args, **kwargs):
    del args, kwargs
    return None


def get_obs_route(*args, **kwargs):
    del args, kwargs
    return None


def get_parameter(*args, **kwargs):
    del args, kwargs
    return None


def get_phrase(*args, **kwargs):
    del args, kwargs
    return None


def get_qcontent(*args, **kwargs):
    del args, kwargs
    return None


def get_qp_ctext(*args, **kwargs):
    del args, kwargs
    return None


def get_quoted_string(*args, **kwargs):
    del args, kwargs
    return None


def get_section(*args, **kwargs):
    del args, kwargs
    return None


def get_token(*args, **kwargs):
    del args, kwargs
    return None


def get_ttext(*args, **kwargs):
    del args, kwargs
    return None


def get_unstructured(*args, **kwargs):
    del args, kwargs
    return None


def get_value(*args, **kwargs):
    del args, kwargs
    return None


def get_word(*args, **kwargs):
    del args, kwargs
    return None


def make_quoted_pairs(*args, **kwargs):
    del args, kwargs
    return None


def parse_content_disposition_header(*args, **kwargs):
    del args, kwargs
    return None


def parse_content_transfer_encoding_header(*args, **kwargs):
    del args, kwargs
    return None


def parse_content_type_header(*args, **kwargs):
    del args, kwargs
    return None


def parse_message_id(*args, **kwargs):
    del args, kwargs
    return None


def parse_mime_parameters(*args, **kwargs):
    del args, kwargs
    return None


def parse_mime_version(*args, **kwargs):
    del args, kwargs
    return None


def quote_string(*args, **kwargs):
    del args, kwargs
    return None


ASPECIALS = set()

ATOM_ENDS = set()

ATTRIBUTE_ENDS = set()

CFWS_LEADER = set()

DOT = ValueTerminal()

DOT_ATOM_ENDS = set()

EXTENDED_ATTRIBUTE_ENDS = set()

ListSeparator = ValueTerminal()

NLSET = set()

PHRASE_ENDS = set()

RouteComponentMarker = ValueTerminal()

SPECIALS = set()

SPECIALSNL = set()

TOKEN_ENDS = set()

TSPECIALS = set()

WSP = set()

errors = _re

hexdigits = "0123456789abcdefABCDEF"

re = _re

rfc2047_matcher = _re.compile("")

sys = _re

urllib = _re

utils = _re
