-- All validations should be done on the Actix API returning the appropriate response codes.
-- Exceptions raised here are considered assertion errors that should not happen in production.

-- New contract verification
CREATE OR REPLACE FUNCTION vsc_cv.can_verify_new(
  _contract_addr VARCHAR,
  _license VARCHAR,
  _lang VARCHAR
)
RETURNS TEXT AS $$
DECLARE
  _status SMALLINT;
BEGIN
  IF (SELECT NOT EXISTS (SELECT 1 FROM vsc_cv.licenses l WHERE l.name = _license)) THEN
    RETURN format('License %s is currently unsupported.', _license);
  ELSIF (SELECT NOT EXISTS (SELECT 1 FROM vsc_cv.languages l WHERE l.name = _lang)) THEN
    RETURN format('Language %s is currently unsupported.', _lang);
  END IF;
  SELECT status INTO _status FROM vsc_cv.contracts c WHERE c.contract_addr = _contract_addr;
  IF _status <> 4 AND _status <> 5 THEN
    RETURN 'Contract is already verified or being verified.';
  ELSE
    RETURN '';
  END IF;
END $$
LANGUAGE plpgsql VOLATILE;

-- Contract code upload
CREATE OR REPLACE FUNCTION vsc_cv.can_upload_file(
  _contract_addr VARCHAR,
  _fname VARCHAR
)
RETURNS TEXT AS $$
DECLARE
  _status SMALLINT;
  _lang SMALLINT;
BEGIN
  IF _fname !~ '^[A-Za-z0-9._-]+$' OR length(_fname) > 50 THEN
    RETURN 'File names must only contain A-Z, a-z, 0-9 and . _ - characters and be less than or equal to 50 characters.';
  END IF;
  SELECT status, lang INTO _status, _lang FROM vsc_cv.contracts WHERE contract_addr = _contract_addr;
  IF _status IS NULL THEN
    RETURN 'Begin contract verification with /verify/new first.';
  ELSIF _status <> 0 THEN
    RETURN format('Status needs to be pending, it is currently %s.', (SELECT name FROM vsc_cv.status WHERE id = _status));
  ELSIF _lang = 0::SMALLINT AND (_fname = 'pnpm-lock.yml' OR _fname = 'pnpm-lock.yaml') THEN
    RETURN 'pnpm-lock.yaml is a reserved filename for pnpm lock files.';
  ELSE
    RETURN '';
  END IF;
END $$
LANGUAGE plpgsql VOLATILE;